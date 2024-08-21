use std::sync::Arc;

use deno_core::url::Url;

use deno_npm::npm_rc::ResolvedNpmRc;
use once_cell::sync::Lazy;

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
