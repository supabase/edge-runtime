// Copyright 2018-2024 the Deno authors. All rights reserved. MIT license.

use std::sync::Arc;

use crate::cache::common::FastInsecureHasher;
use deno_ast::MediaType;
use deno_ast::ModuleSpecifier;
use deno_core::error::AnyError;
use deno_core::serde_json;
use deno_graph::ModuleInfo;
use deno_graph::ModuleParser;
use deno_graph::ParserModuleAnalyzer;
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

pub static MODULE_INFO_CACHE_DB: CacheDBConfiguration = CacheDBConfiguration {
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

pub struct ModuleInfoCacheSourceHash(String);

impl ModuleInfoCacheSourceHash {
    pub fn new(hash: u64) -> Self {
        Self(hash.to_string())
    }

    pub fn from_source(source: &[u8]) -> Self {
        Self::new(FastInsecureHasher::hash(source))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A cache of `deno_graph::ModuleInfo` objects. Using this leads to a considerable
/// performance improvement because when it exists we can skip parsing a module for
/// deno_graph.
pub struct ModuleInfoCache {
    conn: CacheDB,
}

impl ModuleInfoCache {
    #[cfg(test)]
    pub fn new_in_memory(version: &'static str) -> Self {
        Self::new(CacheDB::in_memory(&MODULE_INFO_CACHE_DB, version))
    }

    pub fn new(conn: CacheDB) -> Self {
        Self { conn }
    }

    pub fn get_module_info(
        &self,
        specifier: &ModuleSpecifier,
        media_type: MediaType,
        expected_source_hash: &ModuleInfoCacheSourceHash,
    ) -> Result<Option<ModuleInfo>, AnyError> {
        let query = SELECT_MODULE_INFO;
        let res = self.conn.query_row(
            query,
            params![
                &specifier.as_str(),
                serialize_media_type(media_type),
                expected_source_hash.as_str(),
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
        source_hash: &ModuleInfoCacheSourceHash,
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
                source_hash.as_str(),
                &serde_json::to_string(&module_info)?,
            ],
        )?;
        Ok(())
    }

    pub fn as_module_analyzer<'a>(
        &'a self,
        parser: &'a dyn ModuleParser,
    ) -> ModuleInfoCacheModuleAnalyzer<'a> {
        ModuleInfoCacheModuleAnalyzer {
            module_info_cache: self,
            parser,
        }
    }
}

pub struct ModuleInfoCacheModuleAnalyzer<'a> {
    module_info_cache: &'a ModuleInfoCache,
    parser: &'a dyn ModuleParser,
}

impl<'a> deno_graph::ModuleAnalyzer for ModuleInfoCacheModuleAnalyzer<'a> {
    fn analyze(
        &self,
        specifier: &ModuleSpecifier,
        source: Arc<str>,
        media_type: MediaType,
    ) -> Result<ModuleInfo, deno_ast::ParseDiagnostic> {
        // attempt to load from the cache
        let source_hash = ModuleInfoCacheSourceHash::from_source(source.as_bytes());
        match self
            .module_info_cache
            .get_module_info(specifier, media_type, &source_hash)
        {
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
        let analyzer = ParserModuleAnalyzer::new(self.parser);
        let module_info = analyzer.analyze(specifier, source, media_type)?;

        // then attempt to cache it
        if let Err(err) = self.module_info_cache.set_module_info(
            specifier,
            media_type,
            &source_hash,
            &module_info,
        ) {
            log::debug!(
                "Error saving module cache info for {}. {:#}",
                specifier,
                err
            );
        }

        Ok(module_info)
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
