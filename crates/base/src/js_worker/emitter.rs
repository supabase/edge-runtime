use crate::js_worker::module_loader::make_http_client;
use crate::js_worker::node_module_loader::{CjsResolutionStore, NpmModuleLoader};
use crate::utils::graph_resolver::CliGraphResolver;
use deno_ast::EmitOptions;
use deno_core::error::AnyError;
use deno_npm::NpmSystemInfo;
use eszip::deno_graph::source::{Loader, Resolver};
use module_fetcher::args::lockfile::Lockfile;
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
    NpmPackageFsResolver, NpmResolution,
};
use std::collections::HashMap;
use std::sync::Arc;

pub struct EmitterFactory {
    deno_dir: DenoDir,
}

impl Default for EmitterFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl EmitterFactory {
    pub fn new() -> Self {
        let deno_dir = DenoDir::new(None).unwrap();
        Self { deno_dir }
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
        println!(
            "{}",
            self.deno_dir.npm_folder_path().clone().to_str().unwrap()
        );
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
            None,
            None,
        ))
    }

    pub fn node_resolver(
        &self,
        lock_file: Option<Arc<deno_core::parking_lot::Mutex<Lockfile>>>,
    ) -> Arc<NodeResolver> {
        let fs = self.real_fs().clone();
        Arc::new(NodeResolver::new(fs, self.npm_resolver(lock_file)))
    }

    pub fn npm_module_loader(
        &self,
        lock_file: Option<Arc<deno_core::parking_lot::Mutex<Lockfile>>>,
    ) -> Arc<NpmModuleLoader> {
        let cjs_resolution = Arc::new(CjsResolutionStore::default());
        let cache_db = Caches::new(self.deno_dir_provider());
        let node_analysis_cache = NodeAnalysisCache::new(cache_db.node_analysis_db());
        let cjs_esm_code_analyzer =
            CliCjsCodeAnalyzer::new(node_analysis_cache, self.real_fs().clone());
        let node_code_translator = Arc::new(NodeCodeTranslator::new(
            cjs_esm_code_analyzer,
            self.real_fs().clone(),
            self.node_resolver(lock_file.clone()).clone(),
            self.npm_resolver(lock_file.clone()),
        ));

        Arc::new(NpmModuleLoader::new(
            cjs_resolution,
            node_code_translator,
            self.real_fs(),
            self.node_resolver(lock_file),
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

    pub fn npm_resolver(
        &self,
        lock_file: Option<Arc<deno_core::parking_lot::Mutex<Lockfile>>>,
    ) -> Arc<CliNpmResolver> {
        let npm_resolution = self.npm_resolution();
        let npm_fs_resolver = self.npm_fs();
        Arc::new(CliNpmResolver::new(
            self.real_fs(),
            npm_resolution,
            npm_fs_resolver,
            lock_file,
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
