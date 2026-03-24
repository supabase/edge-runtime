use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::anyhow;
use anyhow::Context;
use deno::deno_npm;
use deno::deno_npm::npm_rc::RegistryConfigWithUrl;
use deno::deno_npm::npm_rc::ResolvedNpmRc;
use deno::deno_path_util::normalize_path;
use deno::standalone::binary::NodeModules;
use deno::standalone::binary::SerializedWorkspaceResolver;
use deno_core::error::AnyError;
use deno_core::serde_json;
use eszip::EszipV2;
use eszip_trait::EszipStaticFiles;
use fs::virtual_fs::VirtualDirectory;
use log::warn;
use rkyv::Archive;
use rkyv::Deserialize;
use rkyv::Serialize;
use url::Url;

#[derive(Debug, Archive, Deserialize, Serialize, Clone)]
#[archive(check_bytes)]
pub enum Entrypoint {
  Key(String),
  ModuleCode(String),
}

#[derive(Default, Debug, Archive, Deserialize, Serialize)]
#[archive(check_bytes)]
#[archive_attr(repr(C))]
pub struct Metadata {
  pub entrypoint: Option<Entrypoint>,
  pub serialized_workspace_resolver_raw: Option<Vec<u8>>,
  pub npmrc_scopes: Option<HashMap<String, String>>,
  pub static_asset_specifiers: Vec<String>,
  pub virtual_dir: Option<VirtualDirectory>,
  pub ca_stores: Option<Vec<String>>,
  pub ca_data: Option<Vec<u8>>,
  pub unsafely_ignore_certificate_errors: Option<Vec<String>>,
  pub node_modules: Option<Vec<u8>>,
}

impl Metadata {
  pub fn serialized_workspace_resolver(
    &self,
  ) -> Result<SerializedWorkspaceResolver, AnyError> {
    Ok(
      self
        .serialized_workspace_resolver_raw
        .as_ref()
        .map(|it| {
          serde_json::from_slice(it.as_slice())
            .context("failed to deserialize workspace resolver from metadata")
        })
        .transpose()?
        .unwrap_or_default(),
    )
  }

  pub fn node_modules(&self) -> Result<Option<NodeModules>, AnyError> {
    self
      .node_modules
      .as_ref()
      .map(|it| {
        serde_json::from_slice(it.as_slice())
          .context("failed to deserialize node modules from metadata")
      })
      .transpose()
  }

  pub fn resolved_npmrc(
    &self,
    registry_yrl: &Url,
  ) -> Result<Arc<ResolvedNpmRc>, AnyError> {
    let scopes = self
      .npmrc_scopes
      .as_ref()
      .map(|it| {
        it.iter()
          .map(
            |(k, v)| -> Result<(String, RegistryConfigWithUrl), AnyError> {
              Ok((
                k.clone(),
                RegistryConfigWithUrl {
                  registry_url: Url::parse(v)
                    .context("failed to parse registry url")?,
                  config: Default::default(),
                },
              ))
            },
          )
          .collect::<Result<HashMap<_, _>, _>>()
      })
      .transpose()?
      .unwrap_or_default();

    Ok(Arc::new(ResolvedNpmRc {
      default_config: deno_npm::npm_rc::RegistryConfigWithUrl {
        registry_url: registry_yrl.clone(),
        config: Default::default(),
      },
      scopes,
      registry_configs: Default::default(),
    }))
  }

  pub fn static_assets_lookup<P>(
    &self,
    mapped_base_dir_path: P,
  ) -> HashMap<PathBuf, String>
  where
    P: AsRef<Path>,
  {
    let mut lookup = EszipStaticFiles::default();

    for specifier in &self.static_asset_specifiers {
      let path = match Url::parse(specifier) {
        Ok(v) => PathBuf::from(v.path()),
        Err(err) => {
          warn!("could not parse the specifier for static file: {}", err);
          continue;
        }
      };

      lookup.insert(
        normalize_path(mapped_base_dir_path.as_ref().join(path)),
        specifier.to_string(),
      );
    }

    lookup
  }

  pub(crate) fn bake(&self, eszip: &mut EszipV2) -> Result<(), AnyError> {
    let Ok(bytes) = rkyv::to_bytes::<_, 1024>(self) else {
      return Err(anyhow!("failed to add metadata into eszip"));
    };

    eszip.add_opaque_data(
      String::from(eszip_trait::v2::METADATA_KEY),
      Arc::from(bytes.into_boxed_slice()),
    );

    Ok(())
  }
}

#[cfg(test)]
mod test {
  use super::*;

  #[test]
  fn metadata_default_has_no_entrypoint() {
    let meta = Metadata::default();
    assert!(meta.entrypoint.is_none());
    assert!(meta.serialized_workspace_resolver_raw.is_none());
    assert!(meta.npmrc_scopes.is_none());
    assert!(meta.static_asset_specifiers.is_empty());
    assert!(meta.virtual_dir.is_none());
    assert!(meta.ca_stores.is_none());
    assert!(meta.ca_data.is_none());
    assert!(meta.node_modules.is_none());
  }

  #[test]
  fn serialized_workspace_resolver_returns_default_when_none() {
    let meta = Metadata::default();
    let result = meta.serialized_workspace_resolver();
    assert!(result.is_ok());
  }

  #[test]
  fn serialized_workspace_resolver_invalid_json_errors() {
    let meta = Metadata {
      serialized_workspace_resolver_raw: Some(b"not valid json".to_vec()),
      ..Default::default()
    };
    let result = meta.serialized_workspace_resolver();
    assert!(result.is_err());
  }

  #[test]
  fn node_modules_returns_none_when_absent() {
    let meta = Metadata::default();
    let result = meta.node_modules().unwrap();
    assert!(result.is_none());
  }

  #[test]
  fn node_modules_invalid_json_errors() {
    let meta = Metadata {
      node_modules: Some(b"bad json".to_vec()),
      ..Default::default()
    };
    assert!(meta.node_modules().is_err());
  }

  #[test]
  fn resolved_npmrc_default_uses_given_registry() {
    let meta = Metadata::default();
    let registry_url = Url::parse("https://registry.npmjs.org/").unwrap();
    let result = meta.resolved_npmrc(&registry_url).unwrap();
    assert_eq!(
      result.default_config.registry_url.as_str(),
      "https://registry.npmjs.org/"
    );
    assert!(result.scopes.is_empty());
  }

  #[test]
  fn resolved_npmrc_with_scopes() {
    let mut scopes = HashMap::new();
    scopes.insert(
      "@myorg".to_string(),
      "https://npm.myorg.com/".to_string(),
    );

    let meta = Metadata {
      npmrc_scopes: Some(scopes),
      ..Default::default()
    };

    let registry_url = Url::parse("https://registry.npmjs.org/").unwrap();
    let result = meta.resolved_npmrc(&registry_url).unwrap();
    assert_eq!(result.scopes.len(), 1);
    assert_eq!(
      result.scopes["@myorg"].registry_url.as_str(),
      "https://npm.myorg.com/"
    );
  }

  #[test]
  fn resolved_npmrc_invalid_scope_url_errors() {
    let mut scopes = HashMap::new();
    scopes.insert("@bad".to_string(), "not-a-url".to_string());

    let meta = Metadata {
      npmrc_scopes: Some(scopes),
      ..Default::default()
    };

    let registry_url = Url::parse("https://registry.npmjs.org/").unwrap();
    assert!(meta.resolved_npmrc(&registry_url).is_err());
  }

  #[test]
  fn static_assets_lookup_empty() {
    let meta = Metadata::default();
    let result = meta.static_assets_lookup("/app");
    assert!(result.is_empty());
  }

  #[test]
  fn static_assets_lookup_maps_specifiers() {
    let meta = Metadata {
      static_asset_specifiers: vec![
        "file:///app/static/index.html".to_string(),
      ],
      ..Default::default()
    };

    let result = meta.static_assets_lookup("/app");
    assert_eq!(result.len(), 1);
    // The key should be a normalized path
    assert!(result.values().any(|v| v.contains("index.html")));
  }

  #[test]
  fn static_assets_lookup_skips_invalid_specifiers() {
    let meta = Metadata {
      static_asset_specifiers: vec![":::invalid".to_string()],
      ..Default::default()
    };

    let result = meta.static_assets_lookup("/app");
    assert!(result.is_empty());
  }

  #[test]
  fn entrypoint_key_debug() {
    let ep = Entrypoint::Key("main.ts".to_string());
    let debug = format!("{:?}", ep);
    assert!(debug.contains("main.ts"));
  }

  #[test]
  fn entrypoint_module_code_debug() {
    let ep = Entrypoint::ModuleCode("console.log('hi')".to_string());
    let debug = format!("{:?}", ep);
    assert!(debug.contains("console.log"));
  }
}
