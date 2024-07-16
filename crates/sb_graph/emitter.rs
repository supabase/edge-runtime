use crate::jsx_util::{get_jsx_emit_opts, get_rt_from_jsx};
use crate::resolver::{
    CjsResolutionStore, CliGraphResolver, CliGraphResolverOptions, CliNodeResolver,
};
use crate::DecoratorType;
use deno_ast::{EmitOptions, SourceMapOption, TranspileOptions};
use deno_cache_dir::HttpCache;
use deno_config::workspace::{PackageJsonDepResolution, WorkspaceResolver};
use deno_config::JsxImportSourceConfig;
use deno_core::error::AnyError;
use deno_core::futures::FutureExt;
use deno_core::parking_lot::Mutex;
use deno_lockfile::Lockfile;
use deno_npm::resolution::ValidSerializedNpmResolutionSnapshot;
use eszip::deno_graph::source::Loader;
use import_map::ImportMap;
use npm_cache::file_fetcher::FileFetcher;
use npm_cache::FetchCacher;
use sb_core::cache::caches::Caches;
use sb_core::cache::deno_dir::{DenoDir, DenoDirProvider};
use sb_core::cache::emit::EmitCache;
use sb_core::cache::fc_permissions::FcPermissions;
use sb_core::cache::module_info::ModuleInfoCache;
use sb_core::cache::parsed_source::ParsedSourceCache;
use sb_core::cache::{CacheSetting, GlobalHttpCache, RealDenoCacheEnv};
use sb_core::emit::Emitter;
use sb_core::npm;
use sb_core::util::http_util::HttpClientProvider;
use sb_node::NodeResolver;

use sb_npm::{
    create_managed_npm_resolver, CliNpmResolver, CliNpmResolverManagedCreateOptions,
    CliNpmResolverManagedSnapshotOption,
};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

struct Deferred<T>(once_cell::unsync::OnceCell<T>);

impl<T> Default for Deferred<T> {
    fn default() -> Self {
        Self(once_cell::unsync::OnceCell::default())
    }
}

impl<T> Deferred<T> {
    #[allow(dead_code)]
    pub fn get_or_try_init(
        &self,
        create: impl FnOnce() -> Result<T, AnyError>,
    ) -> Result<&T, AnyError> {
        self.0.get_or_try_init(create)
    }

    pub fn get_or_init(&self, create: impl FnOnce() -> T) -> &T {
        self.0.get_or_init(create)
    }

    #[allow(dead_code)]
    pub async fn get_or_try_init_async(
        &self,
        create: impl Future<Output = Result<T, AnyError>>,
    ) -> Result<&T, AnyError> {
        if self.0.get().is_none() {
            // todo(dsherret): it would be more ideal if this enforced a
            // single executor and then we could make some initialization
            // concurrent
            let val = create.await?;
            _ = self.0.set(val);
        }
        Ok(self.0.get().unwrap())
    }
}

#[derive(Clone)]
pub struct LockfileOpts {
    path: PathBuf,
    overwrite: bool,
}

pub struct EmitterFactory {
    deno_dir: DenoDir,
    pub npm_snapshot: Option<ValidSerializedNpmResolutionSnapshot>,
    lockfile: Deferred<Option<Arc<Mutex<Lockfile>>>>,
    http_client_provider: Deferred<Arc<HttpClientProvider>>,
    maybe_lockfile: Option<LockfileOpts>,
    maybe_decorator: Option<DecoratorType>,
    cjs_resolutions: Deferred<Arc<CjsResolutionStore>>,
    node_resolver: Deferred<Arc<NodeResolver>>,
    cli_node_resolver: Deferred<Arc<CliNodeResolver>>,
    npm_resolver: Deferred<Arc<dyn CliNpmResolver>>,
    workspace_resolver: Deferred<Arc<WorkspaceResolver>>,
    resolver: Deferred<Arc<CliGraphResolver>>,
    file_fetcher: Deferred<Arc<FileFetcher>>,
    file_fetcher_cache_strategy: Option<CacheSetting>,
    jsx_import_source_config: Option<JsxImportSourceConfig>,
    file_fetcher_allow_remote: bool,
    pub maybe_import_map: Option<ImportMap>,
    module_info_cache: Deferred<Arc<ModuleInfoCache>>,
}

impl Default for EmitterFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl EmitterFactory {
    pub fn new() -> Self {
        let deno_dir = DenoDir::new(None).unwrap();

        Self {
            module_info_cache: Default::default(),
            deno_dir,
            npm_snapshot: None,
            lockfile: Default::default(),
            http_client_provider: Default::default(),
            maybe_lockfile: None,
            maybe_decorator: None,
            cjs_resolutions: Default::default(),
            node_resolver: Default::default(),
            cli_node_resolver: Default::default(),
            npm_resolver: Default::default(),
            workspace_resolver: Default::default(),
            resolver: Default::default(),
            file_fetcher: Default::default(),
            file_fetcher_cache_strategy: None,
            file_fetcher_allow_remote: true,
            maybe_import_map: None,
            jsx_import_source_config: None,
        }
    }

    pub fn set_file_fetcher_cache_strategy(&mut self, strategy: CacheSetting) {
        self.file_fetcher_cache_strategy = Some(strategy);
    }

    pub fn set_file_fetcher_allow_remote(&mut self, allow_remote: bool) {
        self.file_fetcher_allow_remote = allow_remote;
    }

    pub fn set_import_map(&mut self, import_map: Option<ImportMap>) {
        self.maybe_import_map = import_map;
    }

    pub fn set_decorator_type(&mut self, decorator_type: Option<DecoratorType>) {
        self.maybe_decorator = decorator_type;
    }

    pub fn deno_dir_provider(&self) -> Arc<DenoDirProvider> {
        Arc::new(DenoDirProvider::new(None))
    }

    pub fn caches(&self) -> Result<Arc<Caches>, AnyError> {
        let caches = Arc::new(Caches::new(self.deno_dir_provider()));
        let _ = caches.dep_analysis_db();
        let _ = caches.node_analysis_db();
        Ok(caches)
    }

    pub fn module_info_cache(&self) -> Result<&Arc<ModuleInfoCache>, AnyError> {
        self.module_info_cache.get_or_try_init(|| {
            Ok(Arc::new(ModuleInfoCache::new(
                self.caches()?.dep_analysis_db(),
            )))
        })
    }

    pub fn emit_cache(&self) -> Result<EmitCache, AnyError> {
        Ok(EmitCache::new(self.deno_dir.gen_cache.clone()))
    }

    pub fn parsed_source_cache(&self) -> Result<Arc<ParsedSourceCache>, AnyError> {
        let source_cache = Arc::new(ParsedSourceCache::default());
        Ok(source_cache)
    }

    pub fn emit_options(&self) -> EmitOptions {
        EmitOptions {
            inline_sources: true,
            source_map: SourceMapOption::Inline,
            ..Default::default()
        }
    }

    pub fn transpile_options(&self) -> TranspileOptions {
        let (specifier, module) = if let Some(jsx_config) = self.jsx_import_source_config.clone() {
            (jsx_config.default_specifier, jsx_config.module)
        } else {
            (None, "react".to_string())
        };

        let jsx_module = get_rt_from_jsx(Some(module));

        let (transform_jsx, jsx_automatic, jsx_development, precompile_jsx) =
            get_jsx_emit_opts(jsx_module.as_str());

        TranspileOptions {
            use_decorators_proposal: self
                .maybe_decorator
                .map(DecoratorType::is_use_decorators_proposal)
                .unwrap_or_default(),

            use_ts_decorators: self
                .maybe_decorator
                .map(DecoratorType::is_use_ts_decorators)
                .unwrap_or_default(),

            emit_metadata: self
                .maybe_decorator
                .map(DecoratorType::is_emit_metadata)
                .unwrap_or_default(),

            jsx_import_source: specifier,
            transform_jsx,
            jsx_automatic,
            jsx_development,
            precompile_jsx,
            ..Default::default()
        }
    }

    pub fn emitter(&self) -> Result<Arc<Emitter>, AnyError> {
        let emitter = Arc::new(Emitter::new(
            self.emit_cache()?,
            self.parsed_source_cache()?,
            self.transpile_options(),
            self.emit_options(),
        ));

        Ok(emitter)
    }

    pub fn global_http_cache(&self) -> GlobalHttpCache {
        GlobalHttpCache::new(self.deno_dir.deps_folder_path(), RealDenoCacheEnv)
    }

    pub fn http_client_provider(&self) -> &Arc<HttpClientProvider> {
        self.http_client_provider
            .get_or_init(|| Arc::new(HttpClientProvider::new(None, None)))
    }

    pub fn real_fs(&self) -> Arc<dyn deno_fs::FileSystem> {
        Arc::new(deno_fs::RealFs)
    }

    pub fn get_lock_file_deferred(&self) -> &Option<Arc<Mutex<Lockfile>>> {
        self.lockfile.get_or_init(|| {
            if let Some(lockfile_data) = self.maybe_lockfile.clone() {
                Some(Arc::new(Mutex::new(Lockfile::new_empty(
                    lockfile_data.path.clone(),
                    lockfile_data.overwrite,
                ))))
            } else {
                let default_lockfile_path = std::env::current_dir()
                    .map(|p| p.join(".supabase.lock"))
                    .unwrap();
                Some(Arc::new(Mutex::new(Lockfile::new_empty(
                    default_lockfile_path,
                    true,
                ))))
            }
        })
    }

    pub fn get_lock_file(&self) -> Option<Arc<Mutex<Lockfile>>> {
        self.get_lock_file_deferred().as_ref().cloned()
    }

    pub fn cjs_resolutions(&self) -> &Arc<CjsResolutionStore> {
        self.cjs_resolutions.get_or_init(Default::default)
    }

    pub async fn npm_resolver(&self) -> Result<&Arc<dyn CliNpmResolver>, AnyError> {
        self.npm_resolver
            .get_or_try_init_async(async {
                create_managed_npm_resolver(CliNpmResolverManagedCreateOptions {
                    snapshot: CliNpmResolverManagedSnapshotOption::Specified(None),
                    maybe_lockfile: self.get_lock_file(),
                    fs: self.real_fs(),
                    http_client_provider: self.http_client_provider().clone(),
                    npm_global_cache_dir: self.deno_dir.npm_folder_path().clone(),
                    cache_setting: CacheSetting::Use,
                    maybe_node_modules_path: None,
                    npm_system_info: Default::default(),
                    package_json_deps_provider: Default::default(),
                    npmrc: npm::create_default_npmrc(),
                })
                .await
            })
            .await
    }

    pub async fn node_resolver(&self) -> Result<&Arc<NodeResolver>, AnyError> {
        self.node_resolver
            .get_or_try_init_async(
                async {
                    Ok(Arc::new(NodeResolver::new(
                        self.real_fs(),
                        self.npm_resolver().await?.clone().into_npm_resolver(),
                    )))
                }
                .boxed_local(),
            )
            .await
    }

    pub async fn cli_node_resolver(&self) -> Result<&Arc<CliNodeResolver>, AnyError> {
        self.cli_node_resolver
            .get_or_try_init_async(async {
                Ok(Arc::new(CliNodeResolver::new(
                    self.cjs_resolutions().clone(),
                    self.real_fs(),
                    self.node_resolver().await?.clone(),
                    self.npm_resolver().await?.clone(),
                )))
            })
            .await
    }

    pub fn workspace_resolver(&self) -> Result<&Arc<WorkspaceResolver>, AnyError> {
        self.workspace_resolver.get_or_try_init(|| {
            Ok(Arc::new(WorkspaceResolver::new_raw(
                self.maybe_import_map.clone(),
                vec![],
                PackageJsonDepResolution::Disabled,
            )))
        })
    }

    pub async fn cli_graph_resolver(&self) -> &Arc<CliGraphResolver> {
        self.resolver
            .get_or_try_init_async(async {
                Ok(Arc::new(CliGraphResolver::new(CliGraphResolverOptions {
                    node_resolver: Some(self.cli_node_resolver().await?.clone()),
                    npm_resolver: if self.file_fetcher_allow_remote {
                        Some(self.npm_resolver().await?.clone())
                    } else {
                        None
                    },

                    workspace_resolver: self.workspace_resolver()?.clone(),
                    bare_node_builtins_enabled: false,
                    maybe_jsx_import_source_config: self.jsx_import_source_config.clone(),
                    maybe_vendor_dir: None,
                })))
            })
            .await
            .unwrap()
    }

    pub async fn set_jsx_import_source(&mut self, config: JsxImportSourceConfig) {
        self.jsx_import_source_config = Some(config);
    }

    pub fn file_fetcher(&self) -> Result<&Arc<FileFetcher>, AnyError> {
        self.file_fetcher.get_or_try_init(|| {
            let global_cache_struct =
                GlobalHttpCache::new(self.deno_dir.deps_folder_path(), RealDenoCacheEnv);

            let global_cache: Arc<dyn HttpCache> = Arc::new(global_cache_struct);
            let http_client_provider = self.http_client_provider();
            let blob_store = Arc::new(deno_web::BlobStore::default());

            Ok(Arc::new(FileFetcher::new(
                global_cache.clone(),
                self.file_fetcher_cache_strategy
                    .clone()
                    .unwrap_or(CacheSetting::ReloadAll),
                self.file_fetcher_allow_remote,
                http_client_provider.clone(),
                blob_store,
            )))
        })
    }

    pub async fn file_fetcher_loader(&self) -> Result<Box<dyn Loader>, AnyError> {
        let global_cache_struct =
            GlobalHttpCache::new(self.deno_dir.deps_folder_path(), RealDenoCacheEnv);

        Ok(Box::new(FetchCacher::new(
            self.emit_cache()?,
            self.file_fetcher()?.clone(),
            HashMap::new(),
            Arc::new(global_cache_struct),
            self.npm_resolver().await?.clone(),
            self.module_info_cache()?.clone(),
            FcPermissions::allow_all(),
        )))
    }
}
