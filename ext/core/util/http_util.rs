// Copyright 2018-2024 the Deno authors. All rights reserved. MIT license.

use crate::auth_tokens::AuthToken;
use crate::util::versions_util::get_user_agent;
use bytes::Bytes;
use cache_control::Cachability;
use cache_control::CacheControl;
use chrono::DateTime;
use deno_core::error::custom_error;
use deno_core::error::generic_error;
use deno_core::error::AnyError;
use deno_core::parking_lot::Mutex;
use deno_core::url::Url;
use deno_fetch::create_http_client;
use deno_fetch::Client;
use deno_fetch::ClientSendError;
use http;
use http::header::HeaderName;
use http::header::HeaderValue;
use http::header::ACCEPT;
use http::header::AUTHORIZATION;
use http::header::IF_NONE_MATCH;
use http::header::LOCATION;
use http::Response;
use http::StatusCode;
use http_body_util::combinators::BoxBody;
use http_body_util::Empty;
use http_body_util::BodyExt;
use deno_fetch::CreateHttpClientOptions;
use deno_tls::RootCertStoreProvider;
use std::collections::HashMap;
use std::sync::Arc;
use std::thread::ThreadId;
use std::time::Duration;
use std::time::SystemTime;
use thiserror::Error;

/// Construct the next uri based on base uri and location header fragment
/// See <https://tools.ietf.org/html/rfc3986#section-4.2>
fn resolve_url_from_location(base_url: &Url, location: &str) -> Url {
    if location.starts_with("http://") || location.starts_with("https://") {
        // absolute uri
        Url::parse(location).expect("provided redirect url should be a valid url")
    } else if location.starts_with("//") {
        // "//" authority path-abempty
        Url::parse(&format!("{}:{}", base_url.scheme(), location))
            .expect("provided redirect url should be a valid url")
    } else if location.starts_with('/') {
        // path-absolute
        base_url
            .join(location)
            .expect("provided redirect url should be a valid url")
    } else {
        // assuming path-noscheme | path-empty
        let base_url_path_str = base_url.path().to_owned();
        // Pop last part or url (after last slash)
        let segs: Vec<&str> = base_url_path_str.rsplitn(2, '/').collect();
        let new_path = format!("{}/{}", segs.last().unwrap_or(&""), location);
        base_url
            .join(&new_path)
            .expect("provided redirect url should be a valid url")
    }
}

pub fn resolve_redirect_from_response<BodyT>(
    request_url: &Url,
    response: &Response<BodyT>,
) -> Result<Url, DownloadError> {
    debug_assert!(response.status().is_redirection());
    if let Some(location) = response.headers().get(LOCATION) {
        let location_string = location.to_str()?;
        log::debug!("Redirecting to {:?}...", &location_string);
        let new_url = resolve_url_from_location(request_url, location_string);
        Ok(new_url)
    } else {
        Err(DownloadError::NoRedirectHeader {
            request_url: request_url.clone(),
        })
    }
}

// TODO(ry) HTTP headers are not unique key, value pairs. There may be more than
// one header line with the same key. This should be changed to something like
// Vec<(String, String)>
pub type HeadersMap = HashMap<String, String>;

/// A structure used to determine if a entity in the http cache can be used.
///
/// This is heavily influenced by
/// <https://github.com/kornelski/rusty-http-cache-semantics> which is BSD
/// 2-Clause Licensed and copyright Kornel Lesiński
pub struct CacheSemantics {
    cache_control: CacheControl,
    cached: SystemTime,
    headers: HashMap<String, String>,
    now: SystemTime,
}

impl CacheSemantics {
    pub fn new(headers: HashMap<String, String>, cached: SystemTime, now: SystemTime) -> Self {
        let cache_control = headers
            .get("cache-control")
            .map(|v| CacheControl::from_value(v).unwrap_or_default())
            .unwrap_or_default();
        Self {
            cache_control,
            cached,
            headers,
            now,
        }
    }

    fn age(&self) -> Duration {
        let mut age = self.age_header_value();

        if let Ok(resident_time) = self.now.duration_since(self.cached) {
            age += resident_time;
        }

        age
    }

    fn age_header_value(&self) -> Duration {
        Duration::from_secs(
            self.headers
                .get("age")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
        )
    }

    fn is_stale(&self) -> bool {
        self.max_age() <= self.age()
    }

    fn max_age(&self) -> Duration {
        if self.cache_control.cachability == Some(Cachability::NoCache) {
            return Duration::from_secs(0);
        }

        if self.headers.get("vary").map(|s| s.trim()) == Some("*") {
            return Duration::from_secs(0);
        }

        if let Some(max_age) = self.cache_control.max_age {
            return max_age;
        }

        let default_min_ttl = Duration::from_secs(0);

        let server_date = self.raw_server_date();
        if let Some(expires) = self.headers.get("expires") {
            return match DateTime::parse_from_rfc2822(expires) {
                Err(_) => Duration::from_secs(0),
                Ok(expires) => {
                    let expires = SystemTime::UNIX_EPOCH
                        + Duration::from_secs(expires.timestamp().max(0) as _);
                    return default_min_ttl
                        .max(expires.duration_since(server_date).unwrap_or_default());
                }
            };
        }

        if let Some(last_modified) = self.headers.get("last-modified") {
            if let Ok(last_modified) = DateTime::parse_from_rfc2822(last_modified) {
                let last_modified = SystemTime::UNIX_EPOCH
                    + Duration::from_secs(last_modified.timestamp().max(0) as _);
                if let Ok(diff) = server_date.duration_since(last_modified) {
                    let secs_left = diff.as_secs() as f64 * 0.1;
                    return default_min_ttl.max(Duration::from_secs(secs_left as _));
                }
            }
        }

        default_min_ttl
    }

    fn raw_server_date(&self) -> SystemTime {
        self.headers
            .get("date")
            .and_then(|d| DateTime::parse_from_rfc2822(d).ok())
            .and_then(|d| {
                SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(d.timestamp() as _))
            })
            .unwrap_or(self.cached)
    }

    /// Returns true if the cached value is "fresh" respecting cached headers,
    /// otherwise returns false.
    pub fn should_use(&self) -> bool {
        if self.cache_control.cachability == Some(Cachability::NoCache) {
            return false;
        }

        if let Some(max_age) = self.cache_control.max_age {
            if self.age() > max_age {
                return false;
            }
        }

        if let Some(min_fresh) = self.cache_control.min_fresh {
            if self.time_to_live() < min_fresh {
                return false;
            }
        }

        if self.is_stale() {
            let has_max_stale = self.cache_control.max_stale.is_some();
            let allows_stale = has_max_stale
                && self
                    .cache_control
                    .max_stale
                    .map(|val| val > self.age() - self.max_age())
                    .unwrap_or(true);
            if !allows_stale {
                return false;
            }
        }

        true
    }

    fn time_to_live(&self) -> Duration {
        self.max_age().checked_sub(self.age()).unwrap_or_default()
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum FetchOnceResult {
    Code(Vec<u8>, HeadersMap),
    NotModified,
    Redirect(Url, HeadersMap),
    RequestError(String),
    ServerError(StatusCode),
}

#[derive(Debug)]
pub struct FetchOnceArgs {
    pub url: Url,
    pub maybe_accept: Option<String>,
    pub maybe_etag: Option<String>,
    pub maybe_auth_token: Option<AuthToken>,
}

pub struct HttpClientProvider {
    options: CreateHttpClientOptions,
    root_cert_store_provider: Option<Arc<dyn RootCertStoreProvider>>,
    // it's not safe to share a reqwest::Client across tokio runtimes,
    // so we store these Clients keyed by thread id
    // https://github.com/seanmonstar/reqwest/issues/1148#issuecomment-910868788
    #[allow(clippy::disallowed_types)] // reqwest::Client allowed here
    clients_by_thread_id: Mutex<HashMap<ThreadId, Client>>,
}

impl std::fmt::Debug for HttpClientProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpClient")
            .field("options", &self.options)
            .finish()
    }
}

impl HttpClientProvider {
    pub fn new(
        root_cert_store_provider: Option<Arc<dyn RootCertStoreProvider>>,
        unsafely_ignore_certificate_errors: Option<Vec<String>>,
    ) -> Self {
        Self {
            options: CreateHttpClientOptions {
                unsafely_ignore_certificate_errors,
                ..Default::default()
            },
            root_cert_store_provider,
            clients_by_thread_id: Default::default(),
        }
    }

    pub fn get_or_create(&self) -> Result<HttpClient, AnyError> {
        use std::collections::hash_map::Entry;
        let thread_id = std::thread::current().id();
        let mut clients = self.clients_by_thread_id.lock();
        let entry = clients.entry(thread_id);
        match entry {
            Entry::Occupied(entry) => Ok(HttpClient::new(entry.get().clone())),
            Entry::Vacant(entry) => {
                let client = create_http_client(
                    get_user_agent(),
                    CreateHttpClientOptions {
                        root_cert_store: match &self.root_cert_store_provider {
                            Some(provider) => Some(provider.get_or_try_init()?.clone()),
                            None => None,
                        },
                        ..self.options.clone()
                    },
                )?;
                entry.insert(client.clone());
                Ok(HttpClient::new(client))
            }
        }
    }
}

#[derive(Debug, Error)]
#[error("Bad response: {:?}{}", .status_code, .response_text.as_ref().map(|s| format!("\n\n{}", s)).unwrap_or_else(String::new))]
pub struct BadResponseError {
    pub status_code: StatusCode,
    pub response_text: Option<String>,
}

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error(transparent)]
    HttpError(#[from] http::Error),
    #[error(transparent)]
    ToStr(#[from] http::header::ToStrError),
    #[error("Redirection from '{}' did not provide location header", .request_url)]
    NoRedirectHeader { request_url: Url },
    #[error("Too many redirects.")]
    TooManyRedirects,
    #[error(transparent)]
    BadResponse(#[from] BadResponseError),
    #[error(transparent)]
    ClientSendError(#[from] ClientSendError),
    #[error("Unknown error")]
    UnknownError(),
}

#[derive(Debug)]
pub struct HttpClient {
    client: Client,
    // don't allow sending this across threads because then
    // it might be shared accidentally across tokio runtimes
    // which will cause issues
    // https://github.com/seanmonstar/reqwest/issues/1148#issuecomment-910868788
    _unsend_marker: deno_core::unsync::UnsendMarker,
}

impl HttpClient {
    // DO NOT make this public. You should always be creating one of these from
    // the HttpClientProvider
    fn new(client: Client) -> Self {
        Self {
            client,
            _unsend_marker: deno_core::unsync::UnsendMarker::default(),
        }
    }

    pub fn get(&self, url: &Url) -> http::request::Builder {
        http::Request::builder()
            .method("GET")
            .uri(url.as_str())
    }

    pub fn post(&self, url: &Url) -> http::request::Builder {
        http::Request::builder()
            .method("POST")
            .uri(url.as_str())
    }

    fn map_any_error_to_download_error(err: AnyError) -> DownloadError {
        if err.is::<http::Error>() {
            if let Ok(http_error) = err.downcast::<http::Error>() {
                return DownloadError::HttpError(http_error);
            }
        } else if err.is::<http::header::ToStrError>() {
            if let Ok(to_str_error) = err.downcast::<http::header::ToStrError>() {
                return DownloadError::ToStr(to_str_error);
            }
        } else if err.is::<ClientSendError>() {
            if let Ok(client_send_error) = err.downcast::<ClientSendError>() {
                return DownloadError::ClientSendError(client_send_error);
            }
        }
        return DownloadError::UnknownError();
    }

    /// Asynchronously fetches the given HTTP URL one pass only.
    /// If no redirect is present and no error occurs,
    /// yields Code(ResultPayload).
    /// If redirect occurs, does not follow and
    /// yields Redirect(url).
    pub async fn fetch_no_follow(&self, args: FetchOnceArgs) -> Result<FetchOnceResult, AnyError> {
        let mut request = http::Request::builder()
            .method("GET")
            .uri(args.url.as_str());

        if let Some(etag) = args.maybe_etag {
            request = request.header(IF_NONE_MATCH, etag);
        }
        if let Some(auth_token) = args.maybe_auth_token {
            request = request.header(AUTHORIZATION, auth_token.to_string());
        }
        if let Some(accept) = args.maybe_accept {
            request = request.header(ACCEPT, accept);
        }

        let request= request.body(BoxBody::new(Empty::new().map_err(|_infallible_error| generic_error("Infallible error"))))?;

        let response = match self.client.clone().send(request).await {
            Ok(resp) => resp,
            Err(err) => {
                if err.is_connect_error() {
                    return Ok(FetchOnceResult::RequestError(err.to_string()));
                }
                return Err(err.into());
            }
        };

        let status = response.status();
        if status == StatusCode::NOT_MODIFIED {
            return Ok(FetchOnceResult::NotModified);
        }

        let mut result_headers = HashMap::new();
        let response_headers = response.headers();

        if let Some(warning) = response_headers.get("X-Deno-Warning") {
            log::warn!("{}", warning.to_str().unwrap());
        }

        for key in response_headers.keys()  {
            let key_str = key.to_string();
            let values = response_headers.get_all(key);
            let values_str = values
                .iter()
                .map(|e| e.to_str().unwrap().to_string())
                .collect::<Vec<String>>()
                .join(",");
            result_headers.insert(key_str, values_str);
        }

        if status.is_redirection() {
            let new_url = resolve_redirect_from_response(&args.url, &response)?;
            return Ok(FetchOnceResult::Redirect(new_url, result_headers));
        }

        if status.is_server_error() {
            return Ok(FetchOnceResult::ServerError(status));
        }

        if status.is_client_error() {
            let err = if status == StatusCode::NOT_FOUND {
                custom_error(
                    "NotFound",
                    format!("Import '{}' failed, not found.", args.url),
                )
            } else {
                generic_error(format!(
                    "Import '{}' failed: {}",
                    args.url,
                    status
                ))
            };
            return Err(err);
        }

        let body = get_response_body_with_progress(response).await?;

        Ok(FetchOnceResult::Code(body, result_headers))
    }

    pub async fn download_text(&self, url: &Url) -> Result<String, AnyError> {
        let bytes = self.download(url).await?;
        Ok(String::from_utf8(bytes)?)
    }

    pub async fn download(&self, url: &Url) -> Result<Vec<u8>, AnyError> {
        let maybe_bytes = self.download_inner(url, None).await?;
        match maybe_bytes {
            Some(bytes) => Ok(bytes),
            None => Err(custom_error("Http", "Not found.")),
        }
    }

    pub async fn download_with_progress(
        &self,
        url: &Url,
        maybe_header: Option<(HeaderName, HeaderValue)>,
    ) -> Result<Option<Vec<u8>>, DownloadError> {
        self.download_inner(url, maybe_header).await
    }

    pub async fn get_redirected_url(
        &self,
        url: &Url,
        maybe_header: Option<(HeaderName, HeaderValue)>,
    ) -> Result<Url, AnyError> {
        let response = self.get_redirected_response(url, maybe_header).await?;
        Ok(response.extensions().get::<Url>().ok_or(DownloadError::UnknownError())?.clone())
    }

    async fn download_inner(
        &self,
        url: &Url,
        maybe_header: Option<(HeaderName, HeaderValue)>,
    ) -> Result<Option<Vec<u8>>, DownloadError> {
        let response = self.get_redirected_response(url, maybe_header).await?;

        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        } else if !response.status().is_success() {
            let status = response.status();
            let body = get_response_body_with_progress(response)
                .await
                .map_err(Self::map_any_error_to_download_error)?;

            return Err(DownloadError::BadResponse(BadResponseError {
                status_code: status,
                response_text: String::from_utf8(body)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            }));
        }

        get_response_body_with_progress(response)
            .await
            .map(Some)
            .map_err(Self::map_any_error_to_download_error)
    }

    async fn get_redirected_response(
        &self,
        url: &Url,
        mut maybe_header: Option<(HeaderName, HeaderValue)>,
    ) -> Result<Response<BoxBody<Bytes, AnyError>>, DownloadError> {
        let mut current_url = url.clone();
        
        let mut request = http::Request::builder()
            .method("GET")
            .uri(current_url.as_str());

        if let Some((header_name, header_value)) = &maybe_header {
            request = request.header(header_name, header_value);
        }

        let mut request = request.body(BoxBody::new(Empty::new()
            .map_err(|_infallible_error| generic_error("Infallible error"))))?;

        let mut redirect_count = 0;
        loop {
            let response = self.client.clone().send(request)
                .await
                .map_err(|err| DownloadError::ClientSendError(err))?;
            let status = response.status();

            if !status.is_redirection() {
                // Store the final URL in the response extensions
                let mut response = response;
                response.extensions_mut().insert(current_url);
                return Ok(response);
            }

            if redirect_count >= 5 {
                return Err(DownloadError::TooManyRedirects);
            }

            let new_url = resolve_redirect_from_response(&current_url, &response)?;

            let mut request_builder = http::Request::builder()
                .method("GET")
                .uri(new_url.as_str());

            // Only include the header if we're redirecting to the same origin
            if new_url.origin() == current_url.origin() {
                if let Some((header_name, header_value)) = &maybe_header {
                    request_builder = request_builder.header(header_name, header_value);
                }
            } else {
                maybe_header = None;
            }

            request = request_builder.body(BoxBody::new(Empty::new()
                .map_err(|_infallible_error| generic_error("Infallible error"))))?;
            
            current_url = new_url;
            redirect_count += 1;
        }
    }
}

pub async fn get_response_body_with_progress(
    response: Response<BoxBody<Bytes, AnyError>>,
) -> Result<Vec<u8>, AnyError> {
    let body = response.into_body();
    let bytes = body.collect().await?.to_bytes();
    Ok(bytes.to_vec())
}
