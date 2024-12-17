// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

use crate::cache::common::FastInsecureHasher;
use crate::cache::emit::EmitCache;
use crate::cache::parsed_source::ParsedSourceCache;
use deno_ast::TranspileOptions;
use deno_core::error::AnyError;
use deno_core::ModuleCodeString;
use deno_core::ModuleSpecifier;
use deno_graph::MediaType;
use deno_graph::Module;
use deno_graph::ModuleGraph;
use std::sync::Arc;

pub struct Emitter {
    emit_cache: EmitCache,
    parsed_source_cache: Arc<ParsedSourceCache>,
    transpile_and_emit_options: Arc<(deno_ast::TranspileOptions, deno_ast::EmitOptions)>,
    // cached hash of the transpile and emit options
    transpile_and_emit_options_hash: u64,
}

impl Emitter {
    pub fn new(
        emit_cache: EmitCache,
        parsed_source_cache: Arc<ParsedSourceCache>,
        transpile_options: TranspileOptions,
        emit_options: deno_ast::EmitOptions,
    ) -> Self {
        let transpile_and_emit_options_hash = {
            let mut hasher = FastInsecureHasher::new_without_deno_version();
            hasher.write_hashable(&transpile_options);
            hasher.write_hashable(&emit_options);
            hasher.finish()
        };
        Self {
            emit_cache,
            parsed_source_cache,
            transpile_and_emit_options: Arc::new((transpile_options, emit_options)),
            transpile_and_emit_options_hash,
        }
    }

    pub fn cache_module_emits(&self, graph: &ModuleGraph) -> Result<(), AnyError> {
        for module in graph.modules() {
            if let Module::Js(module) = module {
                let is_emittable = matches!(
                    module.media_type,
                    MediaType::TypeScript
                        | MediaType::Mts
                        | MediaType::Cts
                        | MediaType::Jsx
                        | MediaType::Tsx
                );
                if is_emittable {
                    self.emit_parsed_source(&module.specifier, module.media_type, &module.source)?;
                }
            }
        }
        Ok(())
    }

    /// Gets a cached emit if the source matches the hash found in the cache.
    pub fn maybe_cached_emit(&self, specifier: &ModuleSpecifier, source: &str) -> Option<String> {
        let source_hash = self.get_source_hash(source);
        self.emit_cache.get_emit_code(specifier, source_hash)
    }

    pub fn emit_parsed_source(
        &self,
        specifier: &ModuleSpecifier,
        media_type: MediaType,
        source: &Arc<str>,
    ) -> Result<ModuleCodeString, AnyError> {
        let source_hash = self.get_source_hash(source);

        if let Some(emit_code) = self.emit_cache.get_emit_code(specifier, source_hash) {
            Ok(emit_code.into())
        } else {
            // this will use a cached version if it exists
            let parsed_source = self.parsed_source_cache.get_or_parse_module(
                specifier,
                source.clone(),
                media_type,
            )?;

            let transpiled_source = parsed_source.transpile(
                &self.transpile_and_emit_options.0,
                &self.transpile_and_emit_options.1,
            )?;

            let source = transpiled_source.into_source();
            let source_text = String::from_utf8(source.source)?;
            debug_assert!(source.source_map.is_none());
            self.emit_cache
                .set_emit_code(specifier, source_hash, source_text.as_str());
            Ok(source_text.into())
        }
    }

    /// A hashing function that takes the source code and uses the global emit
    /// options then generates a string hash which can be stored to
    /// determine if the cached emit is valid or not.
    fn get_source_hash(&self, source_text: &str) -> u64 {
        FastInsecureHasher::new_without_deno_version()
            .write_str(source_text)
            .write_u64(self.transpile_and_emit_options_hash)
            .finish()
    }
}
