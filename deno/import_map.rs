use anyhow::anyhow;
use anyhow::Error;
use deno_config::workspace::SpecifiedImportMap;
use deno_core::serde_json;
use deno_core::serde_json::json;
use deno_core::serde_json::Value;
use deno_core::url::Url;
use std::fs;
use std::path::Path;
use urlencoding::decode;

/// Creates the default import map with esm.sh -> npm: override.
/// This ensures that modules loaded from esm.sh are redirected to npm:.
fn create_default_import_map() -> Value {
  json!({
    "imports": {
      "https://esm.sh/": "npm:/"
    }
  })
}

/// Merges two import map JSON values, with user_value taking precedence.
/// The user's imports and scopes will override any defaults.
fn merge_import_maps(default_value: Value, user_value: Value) -> Value {
  let mut merged = default_value;

  if let (Some(merged_obj), Some(user_obj)) =
    (merged.as_object_mut(), user_value.as_object())
  {
    for (key, user_val) in user_obj {
      if key == "imports" || key == "scopes" {
        // For imports and scopes, merge the nested objects
        if let Some(merged_section) = merged_obj.get_mut(key) {
          if let (Some(merged_inner), Some(user_inner)) =
            (merged_section.as_object_mut(), user_val.as_object())
          {
            for (inner_key, inner_val) in user_inner {
              merged_inner.insert(inner_key.clone(), inner_val.clone());
            }
          }
        } else {
          // Section doesn't exist in default, just add it
          merged_obj.insert(key.clone(), user_val.clone());
        }
      } else {
        // For other keys, user value takes precedence
        merged_obj.insert(key.clone(), user_val.clone());
      }
    }
  }

  merged
}

pub fn load_import_map(
  maybe_path: Option<&str>,
) -> Result<Option<SpecifiedImportMap>, Error> {
  let default_import_map = create_default_import_map();

  if let Some(path_str) = maybe_path {
    let json_str;
    let base_url;

    // check if the path is a data URI (prefixed with data:)
    // the data URI takes the following format
    // data:{encodeURIComponent(mport_map.json)?{encodeURIComponent(base_path)}
    if path_str.starts_with("data:") {
      let data_uri = Url::parse(path_str)?;
      json_str = decode(data_uri.path())?.into_owned();
      if let Some(query) = data_uri.query() {
        base_url = Url::from_directory_path(decode(query)?.into_owned())
          .map_err(|_| anyhow!("invalid import map base url"))?;
      } else {
        base_url = Url::from_directory_path(std::env::current_dir().unwrap())
          .map_err(|_| anyhow!("invalid import map base url"))?;
      }
    } else {
      let path = Path::new(&path_str);
      let abs_path = std::env::current_dir().map(|p| p.join(path))?;
      json_str = fs::read_to_string(abs_path.clone())?;
      base_url = Url::from_directory_path(abs_path.parent().unwrap())
        .map_err(|_| anyhow!("invalid import map base url"))?;
    }

    let user_value: Value = serde_json::from_str(&json_str)?;
    let value = merge_import_maps(default_import_map, user_value);

    Ok(Some(SpecifiedImportMap { base_url, value }))
  } else {
    // Even without a user import map, return the default with esm.sh -> npm: override
    let base_url = Url::from_directory_path(std::env::current_dir().unwrap())
      .map_err(|_| anyhow!("invalid import map base url"))?;
    Ok(Some(SpecifiedImportMap {
      base_url,
      value: default_import_map,
    }))
  }
}
