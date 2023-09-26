use crate::js_worker::module_loader::make_http_client;
use crate::utils::graph_resolver::CliGraphResolver;
use deno_ast::EmitOptions;
use deno_core::error::AnyError;
use eszip::deno_graph::source::{Loader, Resolver};
use module_fetcher::args::CacheSetting;
use module_fetcher::cache::{
    Caches, DenoDir, DenoDirProvider, EmitCache, GlobalHttpCache, HttpCache, ParsedSourceCache,
};
use module_fetcher::emit::Emitter;
use module_fetcher::file_fetcher::FileFetcher;
use module_fetcher::permissions::Permissions;
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
        Box::new(CliGraphResolver::default())
    }

    pub fn file_fetcher(&self) -> FileFetcher {
        use module_fetcher::cache::*;
        let global_cache_struct = GlobalHttpCache::new(self.deno_dir.deps_folder_path());
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
        let global_cache_struct = GlobalHttpCache::new(self.deno_dir.deps_folder_path());

        Box::new(FetchCacher::new(
            self.emit_cache().unwrap(),
            Arc::new(self.file_fetcher()),
            HashMap::new(),
            Arc::new(global_cache_struct),
            Permissions::allow_all(),
            None, // TODO: NPM
        ))
    }
}
