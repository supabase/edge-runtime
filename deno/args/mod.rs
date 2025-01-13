mod package_json;

use deno_core::parking_lot::Mutex;
use deno_lockfile::Lockfile;
use deno_npm_cache::NpmCacheSetting;
use once_cell::sync::Lazy;
use reqwest::Url;

pub use package_json::NpmInstallDepsProvider;
pub use package_json::PackageJsonDepValueParseWithLocationError;

pub fn jsr_url() -> &'static Url {
  static JSR_URL: Lazy<Url> = Lazy::new(|| {
    let env_var_name = "JSR_URL";
    if let Ok(registry_url) = std::env::var(env_var_name) {
      // ensure there is a trailing slash for the directory
      let registry_url = format!("{}/", registry_url.trim_end_matches('/'));
      match Url::parse(&registry_url) {
        Ok(url) => {
          return url;
        }
        Err(err) => {
          log::debug!(
            "Invalid {} environment variable: {:#}",
            env_var_name,
            err,
          );
        }
      }
    }

    Url::parse("https://jsr.io/").unwrap()
  });

  &JSR_URL
}

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
  pub fn as_npm_cache_setting(&self) -> NpmCacheSetting {
    match self {
      CacheSetting::Only => NpmCacheSetting::Only,
      CacheSetting::ReloadAll => NpmCacheSetting::ReloadAll,
      CacheSetting::ReloadSome(values) => {
        if values.iter().any(|v| v == "npm:") {
          NpmCacheSetting::ReloadAll
        } else {
          NpmCacheSetting::ReloadSome {
            npm_package_names: values
              .iter()
              .filter_map(|v| v.strip_prefix("npm:"))
              .map(|n| n.to_string())
              .collect(),
          }
        }
      }
      CacheSetting::RespectHeaders => unreachable!(), // not supported
      CacheSetting::Use => NpmCacheSetting::Use,
    }
  }
}

pub type CliLockfile = Mutex<Lockfile>;
