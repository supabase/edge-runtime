use crate::js_worker::module_loader::make_http_client;
use crate::js_worker::node_module_loader::{CjsResolutionStore, NpmModuleLoader};
use crate::utils::graph_resolver::{CliGraphResolver, CliGraphResolverOptions};
use deno_ast::EmitOptions;
use deno_core::error::AnyError;
use deno_core::parking_lot::Mutex;
use deno_npm::resolution::ValidSerializedNpmResolutionSnapshot;
use deno_npm::NpmSystemInfo;
use eszip::deno_graph::source::{Loader, Resolver};
use module_fetcher::args::lockfile::{snapshot_from_lockfile, Lockfile};
use module_fetcher::args::package_json::PackageJsonDepsProvider;
use module_fetcher::args::CacheSetting;
use module_fetcher::cache::{
    Caches, DenoDir, DenoDirProvider, EmitCache, GlobalHttpCache, NodeAnalysisCache,
    ParsedSourceCache, RealDenoCacheEnv,
};
use module_fetcher::emit::Emitter;
use module_fetcher::file_fetcher::FileFetcher;
use module_fetcher::http_util::HttpClient;
use module_fetcher::node::CliCjsCodeAnalyzer;
use module_fetcher::permissions::Permissions;
use sb_node::analyze::NodeCodeTranslator;
use sb_node::NodeResolver;
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
    pub fn get_or_try_init(
        &self,
        create: impl FnOnce() -> Result<T, AnyError>,
    ) -> Result<&T, AnyError> {
        self.0.get_or_try_init(create)
    }

    pub fn get_or_init(&self, create: impl FnOnce() -> T) -> &T {
        self.0.get_or_init(create)
    }

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

pub struct EmitterFactory {
    deno_dir: DenoDir,
    npm_snapshot: Option<ValidSerializedNpmResolutionSnapshot>,
    lockfile: Option<Lockfile>,
    cjs_resolutions: Deferred<Arc<CjsResolutionStore>>,
}

impl Default for EmitterFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl EmitterFactory {
    pub fn new() -> Self {
        let deno_dir = DenoDir::new(None).unwrap();
        let lockfile = Lockfile::new(
            PathBuf::from(
                "/Users/andrespirela/Documents/workspace/supabase/edge-runtime/deno.lock",
            ),
            false,
        )
        .unwrap();

        Self {
            deno_dir,
            npm_snapshot: None,
            lockfile: Some(lockfile),
            cjs_resolutions: Default::default(),
        }
    }

    pub async fn npm_snapshot_from_lockfile(&mut self) {
        if let Some(lockfile) = self.lockfile.clone() {
            let npm_api = self.npm_api();
            let snapshot = snapshot_from_lockfile(Arc::new(Mutex::new(lockfile)), &*npm_api)
                .await
                .unwrap();
            self.npm_snapshot = Some(snapshot);
        } else {
            panic!("Lockfile not available");
        }
    }

    pub fn set_npm_snapshot(&mut self, npm_snapshot: Option<ValidSerializedNpmResolutionSnapshot>) {
        self.npm_snapshot = npm_snapshot;
    }

    pub fn set_lockfile(&mut self, lockfile: Option<Lockfile>) {
        self.lockfile = lockfile;
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
        Arc::new(make_http_client().unwrap())
    }

    pub fn real_fs(&self) -> Arc<dyn deno_fs::FileSystem> {
        Arc::new(deno_fs::RealFs)
    }

    pub fn npm_cache(&self) -> Arc<NpmCache> {
        Arc::new(NpmCache::new(
            NpmCacheDir::new(self.deno_dir.npm_folder_path().clone()),
            CacheSetting::Use, // TODO: Maybe ?,
            self.real_fs(),
            self.http_client(),
        ))
    }

    pub fn npm_api(&self) -> Arc<CliNpmRegistryApi> {
        Arc::new(CliNpmRegistryApi::new(
            CliNpmRegistryApi::default_url().to_owned(),
            self.npm_cache(),
            self.http_client(),
        ))
    }

    pub fn npm_resolution(&self) -> Arc<NpmResolution> {
        let npm_registry_api = self.npm_api();
        Arc::new(NpmResolution::from_serialized(
            npm_registry_api.clone(),
            self.npm_snapshot.clone(),
            self.get_lock_file(),
        ))
    }

    pub fn get_lock_file(&self) -> Option<Arc<deno_core::parking_lot::Mutex<Lockfile>>> {
        if self.lockfile.is_some() {
            Some(Arc::new(deno_core::parking_lot::Mutex::new(
                self.lockfile.clone().unwrap(),
            )))
        } else {
            None
        }
    }

    pub fn node_resolver(&self) -> Arc<NodeResolver> {
        let fs = self.real_fs().clone();
        Arc::new(NodeResolver::new(fs, self.npm_resolver()))
    }

    pub fn cjs_resolution_store(&self) -> &Arc<CjsResolutionStore> {
        self.cjs_resolutions.get_or_init(Default::default)
    }

    pub fn npm_module_loader(&self) -> Arc<NpmModuleLoader> {
        let cache_db = Caches::new(self.deno_dir_provider());
        let node_analysis_cache = NodeAnalysisCache::new(cache_db.node_analysis_db());
        let cjs_esm_code_analyzer =
            CliCjsCodeAnalyzer::new(node_analysis_cache, self.real_fs().clone());
        let node_code_translator = Arc::new(NodeCodeTranslator::new(
            cjs_esm_code_analyzer,
            self.real_fs().clone(),
            self.node_resolver().clone(),
            self.npm_resolver(),
        ));

        Arc::new(NpmModuleLoader::new(
            self.cjs_resolution_store().clone(),
            node_code_translator,
            self.real_fs(),
            self.node_resolver(),
        ))
    }

    pub fn npm_fs(&self) -> Arc<dyn NpmPackageFsResolver> {
        let fs = self.real_fs();
        create_npm_fs_resolver(
            fs.clone(),
            self.npm_cache(),
            CliNpmRegistryApi::default_url().to_owned(),
            self.npm_resolution(),
            None,
            NpmSystemInfo::default(),
        )
    }

    pub fn npm_resolver(&self) -> Arc<CliNpmResolver> {
        let npm_resolution = self.npm_resolution();
        let npm_fs_resolver = self.npm_fs();
        Arc::new(CliNpmResolver::new(
            self.real_fs(),
            npm_resolution,
            npm_fs_resolver,
            self.get_lock_file(),
        ))
    }

    pub fn package_json_deps_provider(&self) -> Arc<PackageJsonDepsProvider> {
        Arc::new(PackageJsonDepsProvider::new(None))
    }

    pub fn package_json_deps_installer(&self) -> Arc<PackageJsonDepsInstaller> {
        Arc::new(PackageJsonDepsInstaller::new(
            self.package_json_deps_provider(),
            self.npm_api(),
            self.npm_resolution(),
        ))
    }

    pub fn cli_graph_resolver(&self) -> Arc<CliGraphResolver> {
        Arc::new(CliGraphResolver::new(
            self.npm_api(),
            self.npm_resolution(),
            self.package_json_deps_provider(),
            self.package_json_deps_installer(),
            CliGraphResolverOptions::default(),
        ))
    }

    pub fn file_fetcher(&self) -> FileFetcher {
        use module_fetcher::cache::*;
        let global_cache_struct =
            GlobalHttpCache::new(self.deno_dir.deps_folder_path(), RealDenoCacheEnv);
        let global_cache: Arc<dyn HttpCache> = Arc::new(global_cache_struct);
        let http_client = Arc::new(make_http_client().unwrap());
        let blob_store = Arc::new(deno_web::BlobStore::default());

        FileFetcher::new(
            global_cache.clone(),
            CacheSetting::ReloadAll, // TODO: Maybe ?
            true,
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
