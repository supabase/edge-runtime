use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use deno::deno_npm;
use deno::deno_npm::npm_rc::RegistryConfigWithUrl;
use deno::deno_npm::npm_rc::ResolvedNpmRc;
use deno::deno_path_util::normalize_path;
use deno::standalone::binary::SerializedWorkspaceResolver;
use deno_core::error::AnyError;
use deno_core::serde_json;
use eszip_trait::EszipStaticFiles;
use fs::virtual_fs::VirtualDirectory;
use log::warn;
use rkyv::Archive;
use rkyv::Deserialize;
use rkyv::Serialize;
use url::Url;

#[derive(Default, Archive, Deserialize, Serialize)]
#[archive(check_bytes)]
#[archive_attr(repr(C))]
pub struct Metadata {
  pub module_code: Option<String>,
  pub serialized_workspace_resolver_raw: Option<Vec<u8>>,
  pub npmrc_scopes: Option<HashMap<String, String>>,
  pub static_asset_specifiers: Vec<String>,
  pub vfs: Option<VirtualDirectory>,
  pub ca_stores: Option<Vec<String>>,
  pub ca_data: Option<Vec<u8>>,
  pub unsafely_ignore_certificate_errors: Option<Vec<String>>,
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
          serde_json::from_slice::<SerializedWorkspaceResolver>(it.as_slice())
            .context("failed to deserialize workspace resolver from metadata")
        })
        .transpose()?
        .unwrap_or_default(),
    )
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
}
