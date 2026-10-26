use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use deno_npm::npm_rc::NpmRc;
use deno_npm::npm_rc::ResolvedNpmRc;
use tokio::fs;

use crate::args::npm_registry_url;

pub async fn create_npmrc<P>(
  path: P,
  maybe_env_vars: Option<&HashMap<String, String>>,
) -> Result<Arc<ResolvedNpmRc>, anyhow::Error>
where
  P: AsRef<Path>,
{
  let get_env_fn = |k: &str| {
    #[cfg(feature = "npmrc-env-expansion")]
    {
      maybe_env_vars.as_ref().and_then(|it| it.get(k).cloned())
    }
    #[cfg(not(feature = "npmrc-env-expansion"))]
    {
      let _ = (k, &maybe_env_vars);
      None
    }
  };
  NpmRc::parse(
    fs::read_to_string(path)
      .await
      .context("failed to read path")?
      .as_str(),
    &get_env_fn,
  )
  .context("failed to parse .npmrc file")?
  .as_resolved(npm_registry_url())
  .context("failed to resolve .npmrc file")
  .map(Arc::new)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn env_var_expansion_is_gated_by_feature() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".npmrc");
    fs::write(&path, "//registry.npmjs.org/:_authToken=${NPM_TOKEN}\n")
      .await
      .unwrap();

    let mut env_vars = HashMap::new();
    env_vars.insert("NPM_TOKEN".to_string(), "super-secret".to_string());

    let resolved = create_npmrc(&path, Some(&env_vars)).await.unwrap();
    let auth_token = resolved.default_config.config.auth_token.clone();

    #[cfg(feature = "npmrc-env-expansion")]
    assert_eq!(auth_token.as_deref(), Some("super-secret"));

    #[cfg(not(feature = "npmrc-env-expansion"))]
    assert_eq!(auth_token.as_deref(), Some("${NPM_TOKEN}"));
  }
}
