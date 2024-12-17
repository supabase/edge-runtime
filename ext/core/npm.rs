use std::{collections::HashMap, path::Path, sync::Arc};

use deno_core::url::Url;
use deno_npm::npm_rc::{NpmRc, ResolvedNpmRc};

use anyhow::Context;
use once_cell::sync::Lazy;
use tokio::fs;

pub fn npm_registry_url() -> &'static Url {
    static NPM_REGISTRY_DEFAULT_URL: Lazy<Url> = Lazy::new(|| {
        let env_var_name = "NPM_CONFIG_REGISTRY";
        if let Ok(registry_url) = std::env::var(env_var_name) {
            // ensure there is a trailing slash for the directory
            let registry_url = format!("{}/", registry_url.trim_end_matches('/'));
            match Url::parse(&registry_url) {
                Ok(url) => {
                    return url;
                }
                Err(err) => {
                    log::debug!("Invalid {} environment variable: {:#}", env_var_name, err,);
                }
            }
        }

        Url::parse("https://registry.npmjs.org").unwrap()
    });

    &NPM_REGISTRY_DEFAULT_URL
}

pub fn create_default_npmrc() -> Arc<ResolvedNpmRc> {
    Arc::new(ResolvedNpmRc {
        default_config: deno_npm::npm_rc::RegistryConfigWithUrl {
            registry_url: npm_registry_url().clone(),
            config: Default::default(),
        },
        scopes: Default::default(),
        registry_configs: Default::default(),
    })
}

pub async fn create_npmrc<P>(
    path: P,
    maybe_env_vars: Option<&HashMap<String, String>>,
) -> Result<Arc<ResolvedNpmRc>, anyhow::Error>
where
    P: AsRef<Path>,
{
    let get_env_fn = |k: &str| maybe_env_vars.as_ref().and_then(|it| it.get(k).cloned());
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
