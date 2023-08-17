use deno_ast::EmitOptions;
use deno_core::error::AnyError;
use module_fetcher::cache::{Caches, DenoDir, DenoDirProvider, EmitCache, ParsedSourceCache};
use module_fetcher::emit::Emitter;
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
}
