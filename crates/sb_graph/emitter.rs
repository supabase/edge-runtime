use crate::graph_resolver::{CliGraphResolver, CliGraphResolverOptions};
use crate::jsx_util::{get_jsx_emit_opts, get_rt_from_jsx};
use crate::DecoratorType;
use deno_ast::{EmitOptions, SourceMapOption, TranspileOptions};
use deno_config::JsxImportSourceConfig;
use deno_core::error::AnyError;
use deno_core::parking_lot::Mutex;
use deno_lockfile::Lockfile;
use deno_npm::resolution::ValidSerializedNpmResolutionSnapshot;
use eszip::deno_graph::source::Loader;
use import_map::ImportMap;
use sb_core::cache::caches::Caches;
use sb_core::cache::deno_dir::{DenoDir, DenoDirProvider};
use sb_core::cache::emit::EmitCache;
use sb_core::cache::fc_permissions::FcPermissions;
use sb_core::cache::fetch_cacher::FetchCacher;
use sb_core::cache::module_info::ModuleInfoCache;
use sb_core::cache::parsed_source::ParsedSourceCache;
use sb_core::cache::{CacheSetting, GlobalHttpCache, HttpCache, RealDenoCacheEnv};
use sb_core::emit::Emitter;
use sb_core::file_fetcher::{FileCache, FileFetcher};
use sb_core::util::http_util::HttpClient;
use sb_node::PackageJson;
use sb_npm::cache::NpmCache;
use sb_npm::installer::PackageJsonDepsInstaller;
use sb_npm::package_json::{
    get_local_package_json_version_reqs, PackageJsonDeps, PackageJsonDepsProvider,
};
use sb_npm::registry::CliNpmRegistryApi;
use sb_npm::resolution::NpmResolution;
use sb_npm::{
    create_managed_npm_resolver, CliNpmResolver, CliNpmResolverManagedCreateOptions,
    CliNpmResolverManagedPackageJsonInstallerOption, CliNpmResolverManagedSnapshotOption,
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
    package_json_deps_provider: Deferred<Arc<PackageJsonDepsProvider>>,
    package_json_deps_installer: Deferred<Arc<PackageJsonDepsInstaller>>,
    maybe_package_json_deps: Option<PackageJsonDeps>,
    maybe_lockfile: Option<LockfileOpts>,
    maybe_decorator: Option<DecoratorType>,
    npm_resolver: Deferred<Arc<dyn CliNpmResolver>>,
    resolver: Deferred<Arc<CliGraphResolver>>,
    file_fetcher_cache_strategy: Option<CacheSetting>,
    jsx_import_source_config: Option<JsxImportSourceConfig>,
    file_fetcher_allow_remote: bool,
    pub maybe_import_map: Option<Arc<ImportMap>>,
    file_cache: Deferred<Arc<FileCache>>,
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
            package_json_deps_provider: Default::default(),
            package_json_deps_installer: Default::default(),
            maybe_package_json_deps: None,
            maybe_lockfile: None,
            maybe_decorator: None,
            npm_resolver: Default::default(),
            resolver: Default::default(),
            file_fetcher_cache_strategy: None,
            file_fetcher_allow_remote: true,
            maybe_import_map: None,
            file_cache: Default::default(),
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
        self.maybe_import_map = import_map
            .map(|import_map| Some(Arc::new(import_map)))
            .unwrap_or_else(|| None);
    }

    pub fn set_decorator_type(&mut self, decorator_type: Option<DecoratorType>) {
        self.maybe_decorator = decorator_type;
    }

    pub fn init_package_json_deps(&mut self, package: &PackageJson) {
        self.maybe_package_json_deps = Some(get_local_package_json_version_reqs(package));
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

    pub fn emit_cache(&self, transpile_options: TranspileOptions) -> Result<EmitCache, AnyError> {
        Ok(EmitCache::new(
            self.deno_dir.gen_cache.clone(),
            transpile_options,
        ))
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
        let transpile_options = self.transpile_options();
        let emitter = Arc::new(Emitter::new(
            self.emit_cache(transpile_options.clone())?,
            self.parsed_source_cache()?,
            self.emit_options(),
            transpile_options,
        ));

        Ok(emitter)
    }

    pub fn global_http_cache(&self) -> GlobalHttpCache {
        GlobalHttpCache::new(self.deno_dir.deps_folder_path(), RealDenoCacheEnv)
    }

    pub fn http_client(&self) -> Arc<HttpClient> {
        let root_cert_store = None;
        let unsafely_ignore_certificate_errors = None;

        let http_client = HttpClient::new(root_cert_store, unsafely_ignore_certificate_errors);

        Arc::new(http_client)
    }

    pub fn real_fs(&self) -> Arc<dyn deno_fs::FileSystem> {
        Arc::new(deno_fs::RealFs)
    }

    pub async fn npm_cache(&self) -> Arc<NpmCache> {
        self.npm_resolver()
            .await
            .as_managed()
            .unwrap()
            .npm_cache()
            .clone()
    }

    pub async fn npm_api(&self) -> Arc<CliNpmRegistryApi> {
        self.npm_resolver()
            .await
            .as_managed()
            .unwrap()
            .npm_api()
            .clone()
    }

    pub async fn npm_resolution(&self) -> Arc<NpmResolution> {
        self.npm_resolver()
            .await
            .as_managed()
            .unwrap()
            .npm_resolution()
            .clone()
    }

    pub fn file_cache(&self) -> &Arc<FileCache> {
        self.file_cache.get_or_init(Default::default)
    }

    pub fn get_lock_file_deferred(&self) -> &Option<Arc<Mutex<Lockfile>>> {
        self.lockfile.get_or_init(|| {
            if let Some(lockfile_data) = self.maybe_lockfile.clone() {
                Some(Arc::new(Mutex::new(
                    Lockfile::new(lockfile_data.path.clone(), lockfile_data.overwrite).unwrap(),
                )))
            } else {
                let default_lockfile_path = std::env::current_dir()
                    .map(|p| p.join(".supabase.lock"))
                    .unwrap();
                Some(Arc::new(Mutex::new(
                    Lockfile::new(default_lockfile_path, true).unwrap(),
                )))
            }
        })
    }

    pub fn get_lock_file(&self) -> Option<Arc<Mutex<Lockfile>>> {
        self.get_lock_file_deferred().as_ref().cloned()
    }

    pub async fn npm_resolver(&self) -> &Arc<dyn CliNpmResolver> {
        self.npm_resolver
            .get_or_try_init_async(async {
                create_managed_npm_resolver(CliNpmResolverManagedCreateOptions {
                    snapshot: CliNpmResolverManagedSnapshotOption::Specified(None),
                    maybe_lockfile: self.get_lock_file(),
                    fs: self.real_fs(),
                    http_client: self.http_client(),
                    npm_global_cache_dir: self.deno_dir.npm_folder_path().clone(),
                    cache_setting: CacheSetting::Use,
                    maybe_node_modules_path: None,
                    npm_system_info: Default::default(),
                    package_json_installer:
                        CliNpmResolverManagedPackageJsonInstallerOption::ConditionalInstall(
                            self.package_json_deps_provider().clone(),
                        ),
                    npm_registry_url: CliNpmRegistryApi::default_url().clone(),
                })
                .await
            })
            .await
            .unwrap()
    }

    pub fn package_json_deps_provider(&self) -> &Arc<PackageJsonDepsProvider> {
        self.package_json_deps_provider.get_or_init(|| {
            Arc::new(PackageJsonDepsProvider::new(
                self.maybe_package_json_deps.clone(),
            ))
        })
    }

    pub async fn package_json_deps_installer(&self) -> &Arc<PackageJsonDepsInstaller> {
        self.package_json_deps_installer
            .get_or_try_init_async(async {
                Ok(Arc::new(PackageJsonDepsInstaller::new(
                    self.package_json_deps_provider().clone(),
                    self.npm_api().await.clone(),
                    self.npm_resolution().await.clone(),
                )))
            })
            .await
            .unwrap()
    }

    pub fn cli_graph_resolver_options(&self) -> CliGraphResolverOptions {
        CliGraphResolverOptions {
            maybe_import_map: self.maybe_import_map.clone(),
            maybe_jsx_import_source_config: self.jsx_import_source_config.clone(),
            no_npm: !self.file_fetcher_allow_remote,
            ..Default::default()
        }
    }

    pub async fn set_jsx_import_source(&mut self, config: JsxImportSourceConfig) {
        self.jsx_import_source_config = Some(config);
    }

    pub async fn cli_graph_resolver(&self) -> &Arc<CliGraphResolver> {
        self.resolver
            .get_or_try_init_async(async {
                Ok(Arc::new(CliGraphResolver::new(
                    self.npm_api().await.clone(),
                    self.package_json_deps_provider().clone(),
                    self.package_json_deps_installer().await.clone(),
                    self.cli_graph_resolver_options(),
                    if self.file_fetcher_allow_remote {
                        Some(self.npm_resolver().await.clone())
                    } else {
                        None
                    },
                )))
            })
            .await
            .unwrap()
    }

    pub fn file_fetcher(&self) -> FileFetcher {
        let global_cache_struct =
            GlobalHttpCache::new(self.deno_dir.deps_folder_path(), RealDenoCacheEnv);
        let global_cache: Arc<dyn HttpCache> = Arc::new(global_cache_struct);
        let http_client = self.http_client();
        let blob_store = Arc::new(deno_web::BlobStore::default());

        FileFetcher::new(
            global_cache.clone(),
            self.file_fetcher_cache_strategy
                .clone()
                .unwrap_or(CacheSetting::ReloadAll),
            self.file_fetcher_allow_remote,
            http_client,
            blob_store,
            self.file_cache().clone(),
        )
    }

    pub fn file_fetcher_loader(&self) -> Box<dyn Loader> {
        let global_cache_struct =
            GlobalHttpCache::new(self.deno_dir.deps_folder_path(), RealDenoCacheEnv);

        let parsed_source = self.parsed_source_cache().unwrap();

        Box::new(FetchCacher::new(
            self.module_info_cache().unwrap().clone(),
            self.emit_cache(self.transpile_options()).unwrap(),
            Arc::new(self.file_fetcher()),
            HashMap::new(),
            Arc::new(global_cache_struct),
            parsed_source,
            FcPermissions::allow_all(),
            None, // TODO: NPM
        ))
    }
}
