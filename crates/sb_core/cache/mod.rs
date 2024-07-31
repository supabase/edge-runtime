pub mod cache_db;
pub mod caches;
pub mod check;
pub mod common;
pub mod deno_dir;
pub mod disk_cache;
pub mod emit;
pub mod fc_permissions;
pub mod incremental;
pub mod module_info;
pub mod node;
pub mod parsed_source;

use crate::util::fs::atomic_write_file;
use std::path::Path;
use std::time::SystemTime;

/// Permissions used to save a file in the disk caches.
pub const CACHE_PERM: u32 = 0o644;

/// Indicates how cached source files should be handled.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum CacheSetting {
    /// Only the cached files should be used.  Any files not in the cache will
    /// error.  This is the equivalent of `--cached-only` in the CLI.
    Only,
    /// No cached source files should be used, and all files should be reloaded.
    /// This is the equivalent of `--reload` in the CLI.
    ReloadAll,
    /// Only some cached resources should be used.  This is the equivalent of
    /// `--reload=https://deno.land/std` or
    /// `--reload=https://deno.land/std,https://deno.land/x/example`.
    ReloadSome(Vec<String>),
    /// The usability of a cached value is determined by analyzing the cached
    /// headers and other metadata associated with a cached response, reloading
    /// any cached "non-fresh" cached responses.
    RespectHeaders,
    /// The cached source files should be used for local modules.  This is the
    /// default behavior of the CLI.
    Use,
}

impl CacheSetting {
    pub fn should_use_for_npm_package(&self, package_name: &str) -> bool {
        match self {
            CacheSetting::ReloadAll => false,
            CacheSetting::ReloadSome(list) => {
                if list.iter().any(|i| i == "npm:") {
                    return false;
                }
                let specifier = format!("npm:{package_name}");
                if list.contains(&specifier) {
                    return false;
                }
                true
            }
            _ => true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RealDenoCacheEnv;

impl deno_cache_dir::DenoCacheEnv for RealDenoCacheEnv {
    fn read_file_bytes(&self, path: &Path) -> std::io::Result<Option<Vec<u8>>> {
        match std::fs::read(path) {
            Ok(s) => Ok(Some(s)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn atomic_write_file(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        atomic_write_file(path, bytes, CACHE_PERM)
    }

    fn modified(&self, path: &Path) -> std::io::Result<Option<SystemTime>> {
        match std::fs::metadata(path) {
            Ok(metadata) => Ok(Some(
                metadata.modified().unwrap_or_else(|_| SystemTime::now()),
            )),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn time_now(&self) -> SystemTime {
        SystemTime::now()
    }
}

pub type GlobalHttpCache = deno_cache_dir::GlobalHttpCache<RealDenoCacheEnv>;
pub type LocalHttpCache = deno_cache_dir::LocalHttpCache<RealDenoCacheEnv>;
pub type LocalLspHttpCache = deno_cache_dir::LocalLspHttpCache<RealDenoCacheEnv>;
