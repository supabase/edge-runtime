use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use args::NpmCachingStrategy;
use args::TypeCheckMode;
use deno_config::deno_json::NodeModulesDirMode;
use deno_config::workspace::Workspace;
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
pub use node_resolver;

pub use deno_permissions::PermissionsContainer;

pub fn version() -> &'static str {
  env!("CARGO_PKG_VERSION")
}

pub trait DenoOptions: 'static {
  fn npmrc(&self) -> &Arc<ResolvedNpmRc>;
  fn workspace(&self) -> &Arc<Workspace>;
  fn node_modules_dir_path(&self) -> Option<&PathBuf>;

  fn check_js(&self) -> bool {
    self.workspace().check_js()
  }

  fn default_npm_caching_strategy(&self) -> NpmCachingStrategy {
    NpmCachingStrategy::Eager
  }

  fn type_check_mode(&self) -> TypeCheckMode {
    TypeCheckMode::None
  }

  fn resolve_file_header_overrides(
    &self,
  ) -> HashMap<ModuleSpecifier, HashMap<String, String>> {
    HashMap::new()
  }

  fn to_compiler_option_types(
    &self,
  ) -> Result<Vec<deno_graph::ReferrerImports>, AnyError> {
    todo!()
  }

  fn node_modules_dir(&self) -> Result<Option<NodeModulesDirMode>, AnyError> {
    todo!()
  }

  fn vendor_dir_path(&self) -> Option<&PathBuf> {
    self.workspace().vendor_dir_path()
  }

  fn is_node_main(&self) -> bool {
    todo!()
  }

  fn detect_cjs(&self) -> bool {
    self.workspace().package_jsons().next().is_some() || self.is_node_main()
  }

  fn unstable_detect_cjs(&self) -> bool {
    todo!()
  }

  fn use_byonm(&self) -> bool {
    false
  }
}
