// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

use std::collections::HashMap;
use std::sync::Arc;

use crate::cache::common::FastInsecureHasher;
use deno_ast::MediaType;
use deno_ast::ModuleSpecifier;
use deno_ast::ParsedSource;
use deno_core::error::AnyError;
use deno_core::parking_lot::Mutex;
use deno_core::serde_json;
use deno_graph::CapturingModuleParser;
use deno_graph::DefaultModuleAnalyzer;
use deno_graph::ModuleInfo;
use deno_graph::ModuleParser;
use deno_webstorage::rusqlite::params;

use super::cache_db::CacheDB;
use super::cache_db::CacheDBConfiguration;
use super::cache_db::CacheFailure;

const SELECT_MODULE_INFO: &str = "
SELECT
  module_info
FROM
  moduleinfocache
WHERE
  specifier=?1
  AND media_type=?2
  AND source_hash=?3
LIMIT 1";

pub static PARSED_SOURCE_CACHE_DB: CacheDBConfiguration = CacheDBConfiguration {
    table_initializer: "CREATE TABLE IF NOT EXISTS moduleinfocache (
      specifier TEXT PRIMARY KEY,
      media_type TEXT NOT NULL,
      source_hash TEXT NOT NULL,
      module_info TEXT NOT NULL
    );",
    on_version_change: "DELETE FROM moduleinfocache;",
    preheat_queries: &[SELECT_MODULE_INFO],
    on_failure: CacheFailure::InMemory,
};

#[derive(Clone, Default)]
struct ParsedSourceCacheSources(Arc<Mutex<HashMap<ModuleSpecifier, ParsedSource>>>);

/// It's ok that this is racy since in non-LSP situations
/// this will only ever store one form of a parsed source
/// and in LSP settings the concurrency will be enforced
/// at a higher level to ensure this will have the latest
/// parsed source.
impl deno_graph::ParsedSourceStore for ParsedSourceCacheSources {
    fn set_parsed_source(
        &self,
        specifier: deno_graph::ModuleSpecifier,
        parsed_source: ParsedSource,
    ) -> Option<ParsedSource> {
        self.0.lock().insert(specifier, parsed_source)
    }

    fn get_parsed_source(&self, specifier: &deno_graph::ModuleSpecifier) -> Option<ParsedSource> {
        self.0.lock().get(specifier).cloned()
    }
}

/// A cache of `ParsedSource`s, which may be used with `deno_graph`
/// for cached dependency analysis.
pub struct ParsedSourceCache {
    db: CacheDB,
    sources: ParsedSourceCacheSources,
}

impl ParsedSourceCache {
    #[cfg(test)]
    pub fn new_in_memory() -> Self {
        Self {
            db: CacheDB::in_memory(&PARSED_SOURCE_CACHE_DB, crate::version::deno()),
            sources: Default::default(),
        }
    }

    pub fn new(db: CacheDB) -> Self {
        Self {
            db,
            sources: Default::default(),
        }
    }

    pub fn get_parsed_source_from_esm_module(
        &self,
        module: &deno_graph::EsmModule,
    ) -> Result<ParsedSource, deno_ast::Diagnostic> {
        self.get_or_parse_module(&module.specifier, module.source.clone(), module.media_type)
    }

    /// Gets the matching `ParsedSource` from the cache
    /// or parses a new one and stores that in the cache.
    pub fn get_or_parse_module(
        &self,
        specifier: &deno_graph::ModuleSpecifier,
        source: Arc<str>,
        media_type: MediaType,
    ) -> deno_core::anyhow::Result<ParsedSource, deno_ast::Diagnostic> {
        let parser = self.as_capturing_parser();
        // this will conditionally parse because it's using a CapturingModuleParser
        parser.parse_module(specifier, source, media_type)
    }

    /// Frees the parsed source from memory.
    pub fn free(&self, specifier: &ModuleSpecifier) {
        self.sources.0.lock().remove(specifier);
    }

    pub fn as_analyzer(&self) -> Box<dyn deno_graph::ModuleAnalyzer> {
        Box::new(ParsedSourceCacheModuleAnalyzer::new(
            self.db.clone(),
            self.sources.clone(),
        ))
    }

    /// Creates a parser that will reuse a ParsedSource from the store
    /// if it exists, or else parse.
    pub fn as_capturing_parser(&self) -> CapturingModuleParser {
        CapturingModuleParser::new(None, &self.sources)
    }

    pub fn cache_module_info(
        &self,
        specifier: &ModuleSpecifier,
        media_type: MediaType,
        source: &str,
        module_info: &ModuleInfo,
    ) -> Result<(), AnyError> {
        let source_hash = compute_source_hash(source.as_bytes());
        ParsedSourceCacheModuleAnalyzer::new(self.db.clone(), self.sources.clone()).set_module_info(
            specifier,
            media_type,
            &source_hash,
            module_info,
        )
    }
}

struct ParsedSourceCacheModuleAnalyzer {
    conn: CacheDB,
    sources: ParsedSourceCacheSources,
}

impl ParsedSourceCacheModuleAnalyzer {
    pub fn new(conn: CacheDB, sources: ParsedSourceCacheSources) -> Self {
        Self { conn, sources }
    }

    pub fn get_module_info(
        &self,
        specifier: &ModuleSpecifier,
        media_type: MediaType,
        expected_source_hash: &str,
    ) -> Result<Option<ModuleInfo>, AnyError> {
        let query = SELECT_MODULE_INFO;
        let res = self.conn.query_row(
            query,
            params![
                &specifier.as_str(),
                serialize_media_type(media_type),
                &expected_source_hash,
            ],
            |row| {
                let module_info: String = row.get(0)?;
                let module_info = serde_json::from_str(&module_info)?;
                Ok(module_info)
            },
        )?;
        Ok(res)
    }

    pub fn set_module_info(
        &self,
        specifier: &ModuleSpecifier,
        media_type: MediaType,
        source_hash: &str,
        module_info: &ModuleInfo,
    ) -> Result<(), AnyError> {
        let sql = "
      INSERT OR REPLACE INTO
        moduleinfocache (specifier, media_type, source_hash, module_info)
      VALUES
        (?1, ?2, ?3, ?4)";
        self.conn.execute(
            sql,
            params![
                specifier.as_str(),
                serialize_media_type(media_type),
                &source_hash,
                &serde_json::to_string(&module_info)?,
            ],
        )?;
        Ok(())
    }
}

// todo(dsherret): change this to be stored as an integer next time
// the cache version is bumped
fn serialize_media_type(media_type: MediaType) -> &'static str {
    use MediaType::*;
    match media_type {
        JavaScript => "1",
        Jsx => "2",
        Mjs => "3",
        Cjs => "4",
        TypeScript => "5",
        Mts => "6",
        Cts => "7",
        Dts => "8",
        Dmts => "9",
        Dcts => "10",
        Tsx => "11",
        Json => "12",
        Wasm => "13",
        TsBuildInfo => "14",
        SourceMap => "15",
        Unknown => "16",
    }
}

impl deno_graph::ModuleAnalyzer for ParsedSourceCacheModuleAnalyzer {
    fn analyze(
        &self,
        specifier: &ModuleSpecifier,
        source: Arc<str>,
        media_type: MediaType,
    ) -> Result<ModuleInfo, deno_ast::Diagnostic> {
        // attempt to load from the cache
        let source_hash = compute_source_hash(source.as_bytes());
        match self.get_module_info(specifier, media_type, &source_hash) {
            Ok(Some(info)) => return Ok(info),
            Ok(None) => {}
            Err(err) => {
                log::debug!(
                    "Error loading module cache info for {}. {:#}",
                    specifier,
                    err
                );
            }
        }

        // otherwise, get the module info from the parsed source cache
        let parser = CapturingModuleParser::new(None, &self.sources);
        let analyzer = DefaultModuleAnalyzer::new(&parser);

        let module_info = analyzer.analyze(specifier, source, media_type)?;

        // then attempt to cache it
        if let Err(err) = self.set_module_info(specifier, media_type, &source_hash, &module_info) {
            log::debug!(
                "Error saving module cache info for {}. {:#}",
                specifier,
                err
            );
        }

        Ok(module_info)
    }
}

fn compute_source_hash(bytes: &[u8]) -> String {
    FastInsecureHasher::hash(bytes).to_string()
}
