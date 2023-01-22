// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.
//! This module is meant to eventually implement HTTP cache
//! as defined in RFC 7234 (<https://tools.ietf.org/html/rfc7234>).
//! Currently it's a very simplified version to fulfill Deno needs
//! at hand.
use crate::http_util::HeadersMap;
use crate::util;
use deno_core::error::generic_error;
use deno_core::error::AnyError;
use deno_core::serde::Deserialize;
use deno_core::serde::Serialize;
use deno_core::serde_json;
use deno_core::url::Url;
use log::error;
use std::fs;
use std::fs::File;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;

use super::CACHE_PERM;

/// Turn base of url (scheme, hostname, port) into a valid filename.
/// This method replaces port part with a special string token (because
/// ":" cannot be used in filename on some platforms).
/// Ex: $DENO_DIR/deps/https/deno.land/
fn base_url_to_filename(url: &Url) -> Option<PathBuf> {
    let mut out = PathBuf::new();

    let scheme = url.scheme();
    out.push(scheme);

    match scheme {
        "http" | "https" => {
            let host = url.host_str().unwrap();
            let host_port = match url.port() {
                Some(port) => format!("{}_PORT{}", host, port),
                None => host.to_string(),
            };
            out.push(host_port);
        }
        "data" | "blob" => (),
        scheme => {
            error!("Don't know how to create cache name for scheme: {}", scheme);
            return None;
        }
    };

    Some(out)
}

/// Turn provided `url` into a hashed filename.
/// URLs can contain a lot of characters that cannot be used
/// in filenames (like "?", "#", ":"), so in order to cache
/// them properly they are deterministically hashed into ASCII
/// strings.
///
/// NOTE: this method is `pub` because it's used in integration_tests
pub fn url_to_filename(url: &Url) -> Option<PathBuf> {
    let mut cache_filename = base_url_to_filename(url)?;

    let mut rest_str = url.path().to_string();
    if let Some(query) = url.query() {
        rest_str.push('?');
        rest_str.push_str(query);
    }
    // NOTE: fragment is omitted on purpose - it's not taken into
    // account when caching - it denotes parts of webpage, which
    // in case of static resources doesn't make much sense
    let hashed_filename = util::checksum::gen(&[rest_str.as_bytes()]);
    cache_filename.push(hashed_filename);
    Some(cache_filename)
}

/// Cached metadata about a url.
#[derive(Serialize, Deserialize)]
pub struct CachedUrlMetadata {
    pub headers: HeadersMap,
    pub url: String,
    #[serde(default = "SystemTime::now")]
    pub now: SystemTime,
}

impl CachedUrlMetadata {
    pub fn write(&self, cache_filename: &Path) -> Result<(), AnyError> {
        let metadata_filename = Self::filename(cache_filename);
        let json = serde_json::to_string_pretty(self)?;
        util::fs::atomic_write_file(&metadata_filename, json, CACHE_PERM)?;
        Ok(())
    }

    pub fn read(cache_filename: &Path) -> Result<Self, AnyError> {
        let metadata_filename = Self::filename(cache_filename);
        let metadata = fs::read_to_string(metadata_filename)?;
        let metadata: Self = serde_json::from_str(&metadata)?;
        Ok(metadata)
    }

    /// Ex: $DENO_DIR/deps/https/deno.land/c885b7dcf1d6936e33a9cc3a2d74ec79bab5d733d3701c85a029b7f7ec9fbed4.metadata.json
    pub fn filename(cache_filename: &Path) -> PathBuf {
        cache_filename.with_extension("metadata.json")
    }
}

#[derive(Debug, Clone, Default)]
pub struct HttpCache {
    pub location: PathBuf,
}

impl HttpCache {
    /// Returns a new instance.
    ///
    /// `location` must be an absolute path.
    pub fn new(location: &Path) -> Self {
        assert!(location.is_absolute());
        Self {
            location: location.to_owned(),
        }
    }

    /// Ensures the location of the cache.
    fn ensure_dir_exists(&self, path: &Path) -> io::Result<()> {
        if path.is_dir() {
            return Ok(());
        }
        fs::create_dir_all(path).map_err(|e| {
      io::Error::new(
        e.kind(),
        format!(
          "Could not create remote modules cache location: {:?}\nCheck the permission of the directory.",
          path
        ),
      )
    })
    }

    pub fn get_cache_filename(&self, url: &Url) -> Option<PathBuf> {
        Some(self.location.join(url_to_filename(url)?))
    }

    // TODO(bartlomieju): this method should check headers file
    // and validate against ETAG/Last-modified-as headers.
    // ETAG check is currently done in `cli/file_fetcher.rs`.
    pub fn get(&self, url: &Url) -> Result<(File, HeadersMap, SystemTime), AnyError> {
        let cache_filename = self.location.join(
            url_to_filename(url).ok_or_else(|| generic_error("Can't convert url to filename."))?,
        );
        let metadata_filename = CachedUrlMetadata::filename(&cache_filename);
        let file = File::open(cache_filename)?;
        let metadata = fs::read_to_string(metadata_filename)?;
        let metadata: CachedUrlMetadata = serde_json::from_str(&metadata)?;
        Ok((file, metadata.headers, metadata.now))
    }

    pub fn set(&self, url: &Url, headers_map: HeadersMap, content: &[u8]) -> Result<(), AnyError> {
        let cache_filename = self.location.join(
            url_to_filename(url).ok_or_else(|| generic_error("Can't convert url to filename."))?,
        );
        // Create parent directory
        let parent_filename = cache_filename
            .parent()
            .expect("Cache filename should have a parent dir");
        self.ensure_dir_exists(parent_filename)?;
        // Cache content
        util::fs::atomic_write_file(&cache_filename, content, CACHE_PERM)?;

        let metadata = CachedUrlMetadata {
            now: SystemTime::now(),
            url: url.to_string(),
            headers: headers_map,
        };
        metadata.write(&cache_filename)
    }
}
