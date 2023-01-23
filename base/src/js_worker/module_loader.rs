use anyhow::{bail, Error};
use deno_ast::EmitOptions;
use deno_ast::MediaType;
use deno_ast::ParseParams;
use deno_ast::SourceTextInfo;
use deno_core::error::AnyError;
use deno_core::futures::FutureExt;
use deno_core::ModuleLoader;
use deno_core::ModuleSource;
use deno_core::ModuleSourceFuture;
use deno_core::ModuleSpecifier;
use deno_core::ModuleType;
use deno_core::ResolutionKind;
use module_fetcher::cache::{DenoDir, HttpCache};
use module_fetcher::file_fetcher::{CacheSetting, FileFetcher};
use module_fetcher::http_util::HttpClient;
use std::pin::Pin;

struct ModuleTypeResult {
    module_type: ModuleType,
    should_transpile: bool,
}

fn get_module_type(media_type: MediaType) -> Result<ModuleTypeResult, Error> {
    let (module_type, should_transpile) = match media_type {
        MediaType::JavaScript | MediaType::Mjs | MediaType::Cjs => (ModuleType::JavaScript, false),
        MediaType::Jsx => (ModuleType::JavaScript, true),
        MediaType::TypeScript
        | MediaType::Mts
        | MediaType::Cts
        | MediaType::Dts
        | MediaType::Dmts
        | MediaType::Dcts
        | MediaType::Tsx => (ModuleType::JavaScript, true),
        MediaType::Json => (ModuleType::Json, false),
        _ => bail!("{:?} module type not supported", media_type,),
    };

    Ok(ModuleTypeResult {
        module_type,
        should_transpile,
    })
}

fn make_http_client() -> Result<HttpClient, AnyError> {
    let root_cert_store = None;
    let unsafely_ignore_certificate_errors = None;
    HttpClient::new(root_cert_store, unsafely_ignore_certificate_errors)
}

pub struct DefaultModuleLoader {
    file_fetcher: FileFetcher,
    permissions: module_fetcher::permissions::Permissions,
}

impl DefaultModuleLoader {
    pub fn new() -> Result<Self, AnyError> {
        // Note: we are reusing Deno dependency cache path
        let deno_dir = DenoDir::new(None)?;
        let deps_cache_location = deno_dir.deps_folder_path();

        let http_cache = HttpCache::new(&deps_cache_location);
        let cache_setting = CacheSetting::Use;
        let allow_remote = true;
        let http_client = make_http_client()?;
        let blob_store = deno_web::BlobStore::default();
        let file_fetcher = FileFetcher::new(
            http_cache,
            cache_setting,
            allow_remote,
            http_client,
            blob_store,
        )?;
        let permissions = module_fetcher::permissions::Permissions::default();

        Ok(Self {
            file_fetcher,
            permissions,
        })
    }
}

impl ModuleLoader for DefaultModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, Error> {
        Ok(deno_core::resolve_import(specifier, referrer)?)
    }

    // TODO: implement prepare_load method
    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<ModuleSpecifier>,
        _is_dyn_import: bool,
    ) -> Pin<Box<ModuleSourceFuture>> {
        let file_fetcher = self.file_fetcher.clone();
        let permissions = self.permissions.clone();
        let module_specifier = module_specifier.clone();

        async move {
            let fetched_file = file_fetcher.fetch(&module_specifier, permissions).await?;
            let ModuleTypeResult {
                module_type,
                should_transpile,
            } = get_module_type(fetched_file.media_type)?;

            let code = fetched_file.source.to_string();
            let code = if should_transpile {
                let parsed = deno_ast::parse_module(ParseParams {
                    specifier: module_specifier.to_string(),
                    text_info: SourceTextInfo::from_string(code),
                    media_type: fetched_file.media_type,
                    capture_tokens: false,
                    scope_analysis: false,
                    maybe_syntax: None,
                })?;
                parsed
                    .transpile(&EmitOptions {
                        inline_source_map: true,
                        inline_sources: true,
                        source_map: true,
                        ..Default::default()
                    })?
                    .text
            } else {
                code
            };

            let module = ModuleSource {
                code: code.into_bytes().into_boxed_slice(),
                module_type,
                module_url_specified: module_specifier.to_string(),
                module_url_found: fetched_file.specifier.to_string(),
            };
            Ok(module)
        }
        .boxed_local()
    }
}
