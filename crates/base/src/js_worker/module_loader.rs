use anyhow::{anyhow, bail, Error};
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
use module_fetcher::cache::{DenoDir, GlobalHttpCache, HttpCache};
use module_fetcher::emit::Emitter;
use module_fetcher::file_fetcher::{CacheSetting, FileFetcher};
use module_fetcher::http_util::HttpClient;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use url::Url;

fn get_module_type(media_type: MediaType) -> Result<ModuleType, Error> {
    let module_type = match media_type {
        MediaType::JavaScript | MediaType::Mjs | MediaType::Cjs | MediaType::Unknown => {
            ModuleType::JavaScript
        }
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

pub fn make_http_client() -> Result<HttpClient, AnyError> {
    let root_cert_store = None;
    let unsafely_ignore_certificate_errors = None;
    Ok(HttpClient::new(
        root_cert_store,
        unsafely_ignore_certificate_errors,
    ))
}

pub struct DefaultModuleLoader {
    file_fetcher: FileFetcher,
    permissions: module_fetcher::permissions::Permissions,
    emitter: Arc<Emitter>,
    maybe_import_map: Option<ImportMap>,
}

impl DefaultModuleLoader {
    pub fn new(
        root_path: PathBuf,
        maybe_import_map: Option<ImportMap>,
        emitter: Arc<Emitter>,
        no_cache: bool,
        allow_remote: bool,
    ) -> Result<Self, AnyError> {
        // Note: we are reusing Deno dependency cache path
        let deno_dir = DenoDir::new(None)?;
        let deps_cache_location = deno_dir.deps_folder_path();

        let cache_setting = if no_cache {
            CacheSetting::ReloadAll
        } else {
            CacheSetting::Use
        };
        let http_client = Arc::new(make_http_client()?);
        let blob_store = Arc::new(deno_web::BlobStore::default());

        let global_cache_struct =
            GlobalHttpCache::new(deps_cache_location, module_fetcher::cache::RealDenoCacheEnv);
        let global_cache: Arc<dyn HttpCache> = Arc::new(global_cache_struct);
        let file_fetcher = FileFetcher::new(
            global_cache.clone(),
            cache_setting,
            allow_remote,
            http_client,
            blob_store,
        );
        let permissions = module_fetcher::permissions::Permissions::new(root_path);

        Ok(Self {
            file_fetcher,
            permissions,
            maybe_import_map,
            emitter,
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
            // // Built-in Node modules
            // if let Some(module_name) = specifier.strip_prefix("node:") {
            //     return module_fetcher::node::resolve_builtin_node_module(module_name);
            // }

            deno_core::resolve_import(specifier, referrer).map_err(|err| err.into())
        }
    }

    // TODO: implement prepare_load method
    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<&ModuleSpecifier>,
        _is_dyn_import: bool,
    ) -> Pin<Box<ModuleSourceFuture>> {
        let file_fetcher = self.file_fetcher.clone();
        let permissions = self.permissions.clone();
        let module_specifier = module_specifier.clone();
        let emitter = self.emitter.clone();

        async move {
            let fetched_file = file_fetcher
                .fetch(&module_specifier, permissions)
                .await
                .map_err(|err| {
                    anyhow!(
                        "Failed to load module: {:?} - {:?}",
                        module_specifier.as_str(),
                        err
                    )
                })?;
            let module_type = get_module_type(fetched_file.media_type)?;

            let code = fetched_file.source;
            let code = match fetched_file.media_type {
                MediaType::JavaScript
                | MediaType::Unknown
                | MediaType::Cjs
                | MediaType::Mjs
                | MediaType::Json => code.into(),
                MediaType::Dts | MediaType::Dcts | MediaType::Dmts => Default::default(),
                MediaType::TypeScript
                | MediaType::Mts
                | MediaType::Cts
                | MediaType::Jsx
                | MediaType::Tsx => {
                    emitter.emit_parsed_source(&module_specifier, fetched_file.media_type, &code)?
                }
                MediaType::TsBuildInfo | MediaType::Wasm | MediaType::SourceMap => {
                    panic!("Unexpected media type during import.")
                }
            };

            let module = ModuleSource::new_with_redirect(
                module_type,
                code,
                &module_specifier,
                &fetched_file.specifier,
            );

            Ok(module)
        }
        .boxed_local()
    }
}
