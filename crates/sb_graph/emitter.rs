use crate::graph_resolver::{CliGraphResolver, CliGraphResolverOptions};
use deno_ast::EmitOptions;
use deno_core::error::AnyError;
use deno_core::parking_lot::Mutex;
use deno_npm::resolution::ValidSerializedNpmResolutionSnapshot;
use deno_npm::NpmSystemInfo;
use eszip::deno_graph::source::{Loader, Resolver};
use import_map::ImportMap;
use module_fetcher::args::lockfile::{snapshot_from_lockfile, Lockfile};
use module_fetcher::args::package_json::{
    get_local_package_json_version_reqs, PackageJsonDeps, PackageJsonDepsProvider,
};
use module_fetcher::args::CacheSetting;
use module_fetcher::cache::{
    Caches, DenoDir, DenoDirProvider, EmitCache, GlobalHttpCache, ParsedSourceCache,
    RealDenoCacheEnv,
};
use module_fetcher::emit::Emitter;
use module_fetcher::file_fetcher::FileFetcher;
use module_fetcher::http_util::HttpClient;
use module_fetcher::permissions::Permissions;
use sb_node::{NodeResolver, PackageJson};
use sb_npm::{
    create_npm_fs_resolver, CliNpmRegistryApi, CliNpmResolver, NpmCache, NpmCacheDir,
    NpmPackageFsResolver, NpmResolution, PackageJsonDepsInstaller,
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
    npm_api: Deferred<Arc<CliNpmRegistryApi>>,
    npm_cache: Deferred<Arc<NpmCache>>,
    npm_resolution: Deferred<Arc<NpmResolution>>,
    node_resolver: Deferred<Arc<NodeResolver>>,
    maybe_package_json_deps: Option<PackageJsonDeps>,
    maybe_lockfile: Option<LockfileOpts>,
    npm_resolver: Deferred<Arc<CliNpmResolver>>,
    resolver: Deferred<Arc<CliGraphResolver>>,
    file_fetcher_cache_strategy: Option<CacheSetting>,
    file_fetcher_allow_remote: bool,
    maybe_import_map: Option<Arc<ImportMap>>,
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
            deno_dir,
            npm_snapshot: None,
            lockfile: Default::default(),
            package_json_deps_provider: Default::default(),
            package_json_deps_installer: Default::default(),
            npm_api: Default::default(),
            npm_cache: Default::default(),
            node_resolver: Default::default(),
            npm_resolution: Default::default(),
            maybe_package_json_deps: None,
            maybe_lockfile: None,
            npm_resolver: Default::default(),
            resolver: Default::default(),
            file_fetcher_cache_strategy: None,
            file_fetcher_allow_remote: true,
            maybe_import_map: None,
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

    pub async fn init_npm(&mut self) {
        let _init_lock_file = self.get_lock_file();

        self.npm_snapshot_from_lockfile().await;
    }

    pub async fn npm_snapshot_from_lockfile(&mut self) {
        if let Some(lockfile) = self.get_lock_file().clone() {
            let npm_api = self.npm_api();
            let snapshot = snapshot_from_lockfile(lockfile, &*npm_api.clone())
                .await
                .unwrap();
            self.npm_snapshot = Some(snapshot);
        } else {
            panic!("Lockfile not available");
        }
    }

    pub fn init_package_json_deps(&mut self, package: &PackageJson) {
        self.maybe_package_json_deps = Some(get_local_package_json_version_reqs(package));
    }

    pub fn set_npm_snapshot(&mut self, npm_snapshot: Option<ValidSerializedNpmResolutionSnapshot>) {
        self.npm_snapshot = npm_snapshot;
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

    pub fn emit_cache(&self) -> Result<EmitCache, AnyError> {
        Ok(EmitCache::new(self.deno_dir.gen_cache.clone()))
    }

    pub fn parsed_source_cache(&self) -> Result<Arc<ParsedSourceCache>, AnyError> {
        let source_cache = Arc::new(ParsedSourceCache::new(self.caches()?.dep_analysis_db()));
        Ok(source_cache)
    }

    pub fn emit_options(&self) -> EmitOptions {
        EmitOptions {
            inline_source_map: true,
            inline_sources: true,
            source_map: true,
            ..Default::default()
        }
    }

    pub fn emitter(&self) -> Result<Arc<Emitter>, AnyError> {
        let emitter = Arc::new(Emitter::new(
            self.emit_cache()?,
            self.parsed_source_cache()?,
            self.emit_options(),
        ));

        Ok(emitter)
    }

    pub fn graph_resolver(&self) -> Box<dyn Resolver> {
        Box::<CliGraphResolver>::default()
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

    pub fn npm_cache(&self) -> &Arc<NpmCache> {
        self.npm_cache.get_or_init(|| {
            Arc::new(NpmCache::new(
                NpmCacheDir::new(self.deno_dir.npm_folder_path().clone()),
                CacheSetting::Use, // TODO: Maybe ?,
                self.real_fs(),
                self.http_client(),
            ))
        })
    }

    pub fn npm_api(&self) -> &Arc<CliNpmRegistryApi> {
        self.npm_api.get_or_init(|| {
            Arc::new(CliNpmRegistryApi::new(
                CliNpmRegistryApi::default_url().to_owned(),
                self.npm_cache().clone(),
                self.http_client(),
            ))
        })
    }

    pub fn npm_resolution(&self) -> &Arc<NpmResolution> {
        self.npm_resolution.get_or_init(|| {
            let npm_registry_api = self.npm_api();
            Arc::new(NpmResolution::from_serialized(
                npm_registry_api.clone(),
                self.npm_snapshot.clone(),
                self.get_lock_file(),
            ))
        })
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

    pub fn node_resolver(&self) -> &Arc<NodeResolver> {
        self.node_resolver.get_or_init(|| {
            let fs = self.real_fs().clone();
            Arc::new(NodeResolver::new(fs, self.npm_resolver().clone()))
        })
    }

    pub fn npm_fs(&self) -> Arc<dyn NpmPackageFsResolver> {
        let fs = self.real_fs();
        create_npm_fs_resolver(
            fs.clone(),
            self.npm_cache().clone(),
            CliNpmRegistryApi::default_url().to_owned(),
            self.npm_resolution().clone(),
            None,
            NpmSystemInfo::default(),
        )
    }

    pub fn npm_resolver(&self) -> &Arc<CliNpmResolver> {
        let npm_resolution = self.npm_resolution();
        let npm_fs_resolver = self.npm_fs();
        self.npm_resolver.get_or_init(|| {
            Arc::new(CliNpmResolver::new(
                self.real_fs(),
                npm_resolution.clone(),
                npm_fs_resolver,
                self.get_lock_file(),
            ))
        })
    }

    pub fn package_json_deps_provider(&self) -> &Arc<PackageJsonDepsProvider> {
        self.package_json_deps_provider.get_or_init(|| {
            Arc::new(PackageJsonDepsProvider::new(
                self.maybe_package_json_deps.clone(),
            ))
        })
    }

    pub fn package_json_deps_installer(&self) -> &Arc<PackageJsonDepsInstaller> {
        self.package_json_deps_installer.get_or_init(|| {
            Arc::new(PackageJsonDepsInstaller::new(
                self.package_json_deps_provider().clone(),
                self.npm_api().clone(),
                self.npm_resolution().clone(),
            ))
        })
    }

    pub fn cli_graph_resolver_options(&self) -> CliGraphResolverOptions {
        CliGraphResolverOptions {
            maybe_import_map: self.maybe_import_map.clone(),
            ..Default::default()
        }
    }

    pub fn cli_graph_resolver(&self) -> &Arc<CliGraphResolver> {
        self.resolver.get_or_init(|| {
            Arc::new(CliGraphResolver::new(
                self.npm_api().clone(),
                self.npm_resolution().clone(),
                self.package_json_deps_provider().clone(),
                self.package_json_deps_installer().clone(),
                self.cli_graph_resolver_options(),
            ))
        })
    }

    pub fn file_fetcher(&self) -> FileFetcher {
        use module_fetcher::cache::*;
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
        )
    }

    pub fn file_fetcher_loader(&self) -> Box<dyn Loader> {
        use module_fetcher::cache::*;
        let global_cache_struct =
            GlobalHttpCache::new(self.deno_dir.deps_folder_path(), RealDenoCacheEnv);
        let parsed_source = self.parsed_source_cache().unwrap();

        Box::new(FetchCacher::new(
            self.emit_cache().unwrap(),
            Arc::new(self.file_fetcher()),
            HashMap::new(),
            Arc::new(global_cache_struct),
            parsed_source,
            Permissions::allow_all(),
            None, // TODO: NPM
        ))
    }
}
