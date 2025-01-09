pub mod args;
pub mod auth_tokens;
pub mod cache;
pub mod emit;
pub mod file_fetcher;
pub mod http_util;
pub mod node;
pub mod npm;
pub mod npmrc;
pub mod permissions;
pub mod transpile;
pub mod versions;

pub fn version() -> &'static str {
  env!("CARGO_PKG_VERSION")
}
