use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use args::NpmCachingStrategy;
use args::TypeCheckMode;
use deno_config::deno_json::NodeModulesDirMode;
use deno_config::workspace::CreateResolverOptions;
use deno_config::workspace::PackageJsonDepResolution;
use deno_config::workspace::Workspace;
use deno_config::workspace::WorkspaceResolver;
use deno_core::error::AnyError;
use deno_core::ModuleSpecifier;
use deno_npm::npm_rc::ResolvedNpmRc;

pub mod args;
pub mod auth_tokens;
pub mod cache;
pub mod emit;
pub mod errors;
pub mod file_fetcher;
pub mod graph_util;
pub mod http_util;
pub mod node;
pub mod npm;
pub mod npmrc;
pub mod permissions;
pub mod resolver;
pub mod runtime;
pub mod util;
pub mod versions;

pub use deno_ast;
pub use deno_cache_dir;
pub use deno_config;

pub use deno_crypto;
pub use deno_fetch;
pub use deno_fs;
pub use deno_graph;
pub use deno_http;
pub use deno_io;
pub use deno_lockfile;
pub use deno_net;
pub use deno_npm;
pub use deno_package_json;
pub use deno_path_util;
pub use deno_permissions;
pub use deno_semver;
pub use deno_telemetry;
pub use deno_tls;
pub use deno_url;
pub use deno_web;
pub use deno_webidl;
pub use deno_websocket;
pub use deno_webstorage;

pub use deno_resolver;
use file_fetcher::FileFetcher;
pub use node_resolver;

pub use deno_permissions::PermissionsContainer;

pub fn version() -> &'static str {
  env!("CARGO_PKG_VERSION")
}

pub trait DenoOptions: 'static {
  fn npmrc(&self) -> &Arc<ResolvedNpmRc>;
  fn workspace(&self) -> &Arc<Workspace>;
  fn node_modules_dir_path(&self) -> Option<&PathBuf>;

  fn unstable_detect_cjs(&self) -> bool;
  fn use_byonm(&self) -> bool;
  fn is_node_main(&self) -> bool;
  fn type_check_mode(&self) -> TypeCheckMode;

  fn check_js(&self) -> bool {
    self.workspace().check_js()
  }

  fn default_npm_caching_strategy(&self) -> NpmCachingStrategy {
    NpmCachingStrategy::Eager
  }

  fn resolve_file_header_overrides(
    &self,
  ) -> HashMap<ModuleSpecifier, HashMap<String, String>> {
    HashMap::new()
  }

  fn to_compiler_option_types(
    &self,
  ) -> Result<Vec<deno_graph::ReferrerImports>, AnyError> {
    self
      .workspace()
      .to_compiler_option_types()
      .map(|maybe_imports| {
        maybe_imports
          .into_iter()
          .map(|(referrer, imports)| deno_graph::ReferrerImports {
            referrer,
            imports,
          })
          .collect()
      })
  }

  fn node_modules_dir(&self) -> Result<Option<NodeModulesDirMode>, AnyError> {
    self.workspace().node_modules_dir().map_err(Into::into)
  }

  fn vendor_dir_path(&self) -> Option<&PathBuf> {
    self.workspace().vendor_dir_path()
  }

  fn detect_cjs(&self) -> bool {
    self.workspace().package_jsons().next().is_some() || self.is_node_main()
  }

  fn create_workspace_resolver(
    &self,
    _file_fetcher: &FileFetcher,
    pkg_json_dep_resolution: PackageJsonDepResolution,
  ) -> Result<WorkspaceResolver, AnyError> {
    Ok(self.workspace().create_resolver(
      CreateResolverOptions {
        pkg_json_dep_resolution,
        specified_import_map: None,
      },
      |path| Ok(std::fs::read_to_string(path)?),
    )?)
  }
}
