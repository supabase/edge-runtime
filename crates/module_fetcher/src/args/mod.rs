pub mod config_file;
pub mod flags_net;
pub mod import_map;
pub mod lockfile;
pub mod package_json;

pub use deno_config::BenchConfig;
pub use deno_config::CompilerOptions;
pub use deno_config::ConfigFile;
pub use deno_config::EmitConfigOptions;
pub use deno_config::FilesConfig;
pub use deno_config::FmtOptionsConfig;
pub use deno_config::JsxImportSourceConfig;
pub use deno_config::LintRulesConfig;
pub use deno_config::ProseWrap;
pub use deno_config::TsConfig;
pub use deno_config::TsConfigForEmit;
pub use deno_config::TsConfigType;
pub use deno_config::TsTypeLib;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigFlag {
    Discover,
    Path(String),
    Disabled,
}

impl Default for ConfigFlag {
    fn default() -> Self {
        Self::Discover
    }
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
