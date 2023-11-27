use crate::js_worker::emitter::EmitterFactory;
use crate::js_worker::node_module_loader::ModuleCodeSource;
use crate::utils::graph_util::{create_graph, create_graph_from_specifiers};
use anyhow::{anyhow, Context, Error};
use deno_ast::MediaType;
use deno_core::error::{custom_error, AnyError};
use deno_core::futures::Future;
use deno_core::futures::FutureExt;
use deno_core::ModuleSource;
use deno_core::ModuleSourceFuture;
use deno_core::ModuleSpecifier;
use deno_core::ModuleType;
use deno_core::ResolutionKind;
use deno_core::{ModuleCode, ModuleLoader};
use eszip::deno_graph;
use eszip::deno_graph::source::Resolver;
use eszip::deno_graph::{EsmModule, JsonModule, Module, ModuleGraph, Resolution};
use import_map::ImportMap;
use module_fetcher::file_fetcher::CacheSetting;
use module_fetcher::http_util::HttpClient;
use module_fetcher::node;
use module_fetcher::util::text_encoding::code_without_source_map;
use std::pin::Pin;
use std::sync::Arc;

pub fn make_http_client() -> Result<HttpClient, AnyError> {
    let root_cert_store = None;
    let unsafely_ignore_certificate_errors = None;
    Ok(HttpClient::new(
        root_cert_store,
        unsafely_ignore_certificate_errors,
    ))
}

struct PreparedModuleLoader {
    graph: ModuleGraph,
    emitter: Arc<EmitterFactory>,
}

impl PreparedModuleLoader {
    pub fn load_prepared_module(
        &self,
        specifier: &ModuleSpecifier,
        maybe_referrer: Option<&ModuleSpecifier>,
    ) -> Result<ModuleCodeSource, AnyError> {
        if specifier.scheme() == "node" {
            unreachable!(); // Node built-in modules should be handled internally.
        }

        match self.graph.get(specifier) {
            Some(deno_graph::Module::Json(JsonModule {
                source,
                media_type,
                specifier,
                ..
            })) => Ok(ModuleCodeSource {
                code: source.clone().into(),
                found_url: specifier.clone(),
                media_type: *media_type,
            }),
            Some(deno_graph::Module::Esm(EsmModule {
                source,
                media_type,
                specifier,
                ..
            })) => {
                let code: ModuleCode = match media_type {
                    MediaType::JavaScript
                    | MediaType::Unknown
                    | MediaType::Cjs
                    | MediaType::Mjs
                    | MediaType::Json => source.clone().into(),
                    MediaType::Dts | MediaType::Dcts | MediaType::Dmts => Default::default(),
                    MediaType::TypeScript
                    | MediaType::Mts
                    | MediaType::Cts
                    | MediaType::Jsx
                    | MediaType::Tsx => {
                        // get emit text
                        self.emitter.emitter().unwrap().emit_parsed_source(
                            specifier,
                            *media_type,
                            source,
                        )?
                    }
                    MediaType::TsBuildInfo | MediaType::Wasm | MediaType::SourceMap => {
                        panic!("Unexpected media type {media_type} for {specifier}")
                    }
                };

                Ok(ModuleCodeSource {
                    code,
                    found_url: specifier.clone(),
                    media_type: *media_type,
                })
            }
            _ => {
                let mut msg = format!("Loading unprepared module: {specifier}");
                if let Some(referrer) = maybe_referrer {
                    msg = format!("{}, imported from: {}", msg, referrer.as_str());
                }
                Err(anyhow!(msg))
            }
        }
    }

    pub async fn prepare_module_load(
        &self,
        roots: Vec<ModuleSpecifier>,
        is_dynamic: bool,
    ) -> Result<(), AnyError> {
        create_graph_from_specifiers(roots, is_dynamic, self.emitter.clone()).await?;

        // If there is a lockfile...
        if let Some(lockfile) = self.emitter.get_lock_file() {
            let lockfile = lockfile.lock();
            // update it with anything new
            lockfile.write().context("Failed writing lockfile.")?;
        }

        Ok(())
    }
}

pub struct DefaultModuleLoader {
    graph: ModuleGraph,
    prepared_module_loader: Arc<PreparedModuleLoader>,
    emitter: Arc<EmitterFactory>,
}

impl DefaultModuleLoader {
    #[allow(clippy::arc_with_non_send_sync)]
    pub async fn new(
        main_module: ModuleSpecifier,
        maybe_import_map: Option<ImportMap>,
        mut emitter: EmitterFactory,
        no_cache: bool,
        allow_remote: bool,
    ) -> Result<Self, AnyError> {
        let cache_setting = if no_cache {
            CacheSetting::ReloadAll
        } else {
            CacheSetting::Use
        };

        emitter.set_file_fetcher_cache_strategy(cache_setting);
        emitter.set_file_fetcher_allow_remote(allow_remote);
        emitter.set_import_map(maybe_import_map);

        let emitter = Arc::new(emitter);
        let graph = create_graph(main_module.to_file_path().unwrap(), emitter.clone()).await;

        Ok(Self {
            graph: graph.clone(),
            prepared_module_loader: Arc::new(PreparedModuleLoader {
                graph,
                emitter: emitter.clone(),
            }),
            emitter,
        })
    }

    fn load_sync(
        &self,
        specifier: &ModuleSpecifier,
        maybe_referrer: Option<&ModuleSpecifier>,
        _is_dynamic: bool,
    ) -> Result<ModuleSource, AnyError> {
        let code_source = if let Some(result) = self
            .emitter
            .npm_module_loader()
            .load_sync_if_in_npm_package(specifier, maybe_referrer, &*sb_node::allow_all())
        {
            result?
        } else {
            self.prepared_module_loader
                .load_prepared_module(specifier, maybe_referrer)?
        };

        let code = code_without_source_map(code_source.code);

        Ok(ModuleSource::new_with_redirect(
            match code_source.media_type {
                MediaType::Json => ModuleType::Json,
                _ => ModuleType::JavaScript,
            },
            code,
            specifier,
            &code_source.found_url,
        ))
    }
}

impl ModuleLoader for DefaultModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, Error> {
        let cwd = std::env::current_dir().context("Unable to get CWD")?;
        let referrer_result = deno_core::resolve_url_or_path(referrer, &cwd);
        let permissions = sb_node::allow_all();
        let npm_module_loader = self.emitter.npm_module_loader();

        if let Ok(referrer) = referrer_result.as_ref() {
            if let Some(result) =
                npm_module_loader.resolve_if_in_npm_package(specifier, referrer, &*permissions)
            {
                return result;
            }

            let graph = self.graph.clone();
            let maybe_resolved = match graph.get(referrer) {
                Some(Module::Esm(module)) => {
                    module.dependencies.get(specifier).map(|d| &d.maybe_code)
                }
                _ => None,
            };

            match maybe_resolved {
                Some(Resolution::Ok(resolved)) => {
                    let specifier = &resolved.specifier;

                    return match graph.get(specifier) {
                        Some(Module::Npm(module)) => {
                            npm_module_loader.resolve_nv_ref(&module.nv_reference, &*permissions)
                        }
                        Some(Module::Node(module)) => Ok(module.specifier.clone()),
                        Some(Module::Esm(module)) => Ok(module.specifier.clone()),
                        Some(Module::Json(module)) => Ok(module.specifier.clone()),
                        Some(Module::External(module)) => {
                            Ok(node::resolve_specifier_into_node_modules(&module.specifier))
                        }
                        None => Ok(specifier.clone()),
                    };
                }
                Some(Resolution::Err(err)) => {
                    return Err(custom_error(
                        "TypeError",
                        format!("{}\n", err.to_string_with_range()),
                    ))
                }
                Some(Resolution::None) | None => {}
            }
        }

        self.emitter
            .cli_graph_resolver()
            .clone()
            .resolve(specifier, &referrer_result?)
    }

    fn prepare_load(
        &self,
        specifier: &ModuleSpecifier,
        _maybe_referrer: Option<String>,
        is_dynamic: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), AnyError>>>> {
        if let Some(result) = self
            .emitter
            .npm_module_loader()
            .maybe_prepare_load(specifier)
        {
            return Box::pin(deno_core::futures::future::ready(result));
        }

        let specifier = specifier.clone();
        let module_load_preparer = self.prepared_module_loader.clone();

        async move {
            module_load_preparer
                .prepare_module_load(vec![specifier], is_dynamic)
                .await
        }
        .boxed_local()
    }

    // TODO: implement prepare_load method
    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<&ModuleSpecifier>,
        _is_dyn_import: bool,
    ) -> Pin<Box<ModuleSourceFuture>> {
        Box::pin(deno_core::futures::future::ready(self.load_sync(
            module_specifier,
            _maybe_referrer,
            _is_dyn_import,
        )))
    }
}
