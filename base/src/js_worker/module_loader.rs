use anyhow::{bail, Error};
use deno_ast::EmitOptions;
use deno_ast::MediaType;
use deno_core::error::AnyError;
use deno_core::futures::FutureExt;
use deno_core::ModuleLoader;
use deno_core::ModuleSource;
use deno_core::ModuleSourceFuture;
use deno_core::ModuleSpecifier;
use deno_core::ModuleType;
use deno_core::ResolutionKind;
use import_map::ImportMap;
use module_fetcher::cache::{DenoDir, EmitCache, FastInsecureHasher, HttpCache, ParsedSourceCache};
use module_fetcher::emit::emit_parsed_source;
use module_fetcher::file_fetcher::{CacheSetting, FileFetcher};
use module_fetcher::http_util::HttpClient;
use std::path::Path;
use std::pin::Pin;
use url::Url;

fn get_module_type(media_type: MediaType) -> Result<ModuleType, Error> {
    let module_type = match media_type {
        MediaType::JavaScript | MediaType::Mjs | MediaType::Cjs => ModuleType::JavaScript,
        MediaType::Jsx => ModuleType::JavaScript,
        MediaType::TypeScript
        | MediaType::Mts
        | MediaType::Cts
        | MediaType::Dts
        | MediaType::Dmts
        | MediaType::Dcts
        | MediaType::Tsx => ModuleType::JavaScript,
        MediaType::Json => ModuleType::Json,
        _ => bail!("{:?} module type not supported", media_type,),
    };

    Ok(module_type)
}

fn make_http_client() -> Result<HttpClient, AnyError> {
    let root_cert_store = None;
    let unsafely_ignore_certificate_errors = None;
    HttpClient::new(root_cert_store, unsafely_ignore_certificate_errors)
}

pub struct DefaultModuleLoader {
    file_fetcher: FileFetcher,
    permissions: module_fetcher::permissions::Permissions,
    emit_cache: EmitCache,
    parsed_source_cache: ParsedSourceCache,
    maybe_import_map: Option<ImportMap>,
}

impl DefaultModuleLoader {
    pub fn new(maybe_import_map: Option<ImportMap>, no_cache: bool) -> Result<Self, AnyError> {
        // Note: we are reusing Deno dependency cache path
        let deno_dir = DenoDir::new(None)?;
        let deps_cache_location = deno_dir.deps_folder_path();

        let http_cache = HttpCache::new(&deps_cache_location);
        let cache_setting = if no_cache {
            CacheSetting::ReloadAll
        } else {
            CacheSetting::Use
        };
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
        let emit_cache = EmitCache::new(deno_dir.gen_cache.clone());
        let parsed_source_cache =
            ParsedSourceCache::new(Some(deno_dir.dep_analysis_db_file_path()));

        Ok(Self {
            file_fetcher,
            permissions,
            emit_cache,
            parsed_source_cache,
            maybe_import_map,
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
        if let Some(import_map) = &self.maybe_import_map {
            let referrer_relative = Path::new(referrer).is_relative();
            let referrer_url = if referrer_relative {
                import_map.base_url().join(referrer)
            } else {
                Url::parse(referrer)
            };
            if referrer_url.is_err() {
                return referrer_url.map_err(|err| err.into());
            }

            let referrer_url = referrer_url.unwrap();
            import_map
                .resolve(specifier, &referrer_url)
                .map_err(|err| err.into())
        } else {
            deno_core::resolve_import(specifier, referrer).map_err(|err| err.into())
        }
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
        let emit_cache = self.emit_cache.clone();
        let parsed_source_cache = self.parsed_source_cache.clone();
        let emit_options = EmitOptions {
            inline_source_map: true,
            inline_sources: true,
            source_map: true,
            ..Default::default()
        };
        let emit_config_hash = FastInsecureHasher::new()
            .write_hashable(&emit_options)
            .finish();

        async move {
            let fetched_file = file_fetcher.fetch(&module_specifier, permissions).await?;
            let module_type = get_module_type(fetched_file.media_type)?;

            let code = fetched_file.source;
            let code = emit_parsed_source(
                &emit_cache,
                &parsed_source_cache,
                &module_specifier,
                fetched_file.media_type,
                &code,
                &emit_options,
                emit_config_hash,
            )?;

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
