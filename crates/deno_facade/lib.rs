use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use ::eszip::EszipV2;
use deno_core::url::Url;
use eszip::extract_eszip;
use eszip::ExtractEszipPayload;
use serde::Deserialize;
use serde::Serialize;
use tokio::fs::create_dir_all;

mod emitter;
mod eszip;

pub mod errors;
pub mod graph;
pub mod import_map;
pub mod jsr;
pub mod jsx_util;
pub mod module_loader;
pub mod permissions;

pub use ::eszip::v2::Checksum;
pub use emitter::EmitterFactory;
pub use eszip::generate_binary_eszip;
pub use eszip::include_glob_patterns_in_eszip;
pub use eszip::migrate;
pub use eszip::payload_to_eszip;
pub use eszip::EszipPayloadKind;
pub use eszip::LazyLoadableEszip;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecoratorType {
  /// Use TC39 Decorators Proposal - https://github.com/tc39/proposal-decorators
  Tc39,
  /// Use TypeScript experimental decorators.
  Typescript,
  /// Use TypeScript experimental decorators. It also emits metadata.
  TypescriptWithMetadata,
}

impl Default for DecoratorType {
  fn default() -> Self {
    Self::Typescript
  }
}

impl DecoratorType {
  fn is_use_decorators_proposal(self) -> bool {
    matches!(self, Self::Tc39)
  }

  fn is_use_ts_decorators(self) -> bool {
    matches!(self, Self::Typescript | Self::TypescriptWithMetadata)
  }

  fn is_emit_metadata(self) -> bool {
    matches!(self, Self::TypescriptWithMetadata)
  }
}

fn ensure_unix_relative_path(path: &Path) -> &Path {
  assert!(path.is_relative());
  assert!(!path.to_string_lossy().starts_with('\\'));
  path
}

async fn create_module_path(
  global_specifier: &str,
  entry_path: &Path,
  output_folder: &Path,
) -> PathBuf {
  let cleaned_specifier =
    global_specifier.replace(entry_path.to_str().unwrap(), "");
  let module_path = PathBuf::from(cleaned_specifier);

  if let Some(parent) = module_path.parent() {
    if parent.parent().is_some() {
      let output_folder_and_mod_folder = output_folder.join(
        parent
          .strip_prefix("/")
          .unwrap_or_else(|_| ensure_unix_relative_path(parent)),
      );
      if !output_folder_and_mod_folder.exists() {
        create_dir_all(&output_folder_and_mod_folder).await.unwrap();
      }
    }
  }

  output_folder.join(
    module_path
      .strip_prefix("/")
      .unwrap_or_else(|_| ensure_unix_relative_path(&module_path)),
  )
}

async fn extract_modules(
  eszip: &EszipV2,
  specifiers: &[String],
  lowest_path: &str,
  output_folder: &Path,
) {
  let main_path = PathBuf::from(lowest_path);
  let entry_path = main_path.parent().unwrap();
  for global_specifier in specifiers {
    let module_path =
      create_module_path(global_specifier, entry_path, output_folder).await;
    let module_content = eszip
      .get_module(global_specifier)
      .unwrap()
      .take_source()
      .await
      .unwrap();

    let mut file = File::create(&module_path).unwrap();
    file.write_all(module_content.as_ref()).unwrap();
  }
}

pub async fn extract_from_file(
  eszip_file: PathBuf,
  output_path: PathBuf,
) -> bool {
  let eszip_content = std::fs::read(eszip_file).expect("File does not exist");

  extract_eszip(ExtractEszipPayload {
    data: EszipPayloadKind::VecKind(eszip_content),
    folder: output_path,
  })
  .await
}

#[cfg(test)]
mod test {
  use std::fs::remove_dir_all;
  use std::path::PathBuf;
  use std::sync::Arc;

  use deno::DenoOptionsBuilder;

  use crate::emitter::EmitterFactory;
  use crate::eszip::extract_eszip;
  use crate::eszip::generate_binary_eszip;
  use crate::eszip::EszipPayloadKind;
  use crate::eszip::ExtractEszipPayload;

  #[tokio::test]
  #[allow(clippy::arc_with_non_send_sync)]
  async fn test_module_code_no_eszip() {
    let mut emitter_factory = EmitterFactory::new();

    emitter_factory.set_deno_options(
      DenoOptionsBuilder::new()
        .entrypoint(PathBuf::from("./test_cases/npm/index.ts"))
        .build()
        .unwrap(),
    );

    let eszip =
      generate_binary_eszip(Arc::new(emitter_factory), None, None, None)
        .await
        .unwrap();

    assert!(
      extract_eszip(ExtractEszipPayload {
        data: EszipPayloadKind::Eszip(eszip),
        folder: PathBuf::from("../test_cases/npm/"),
      })
      .await
    );

    assert!(PathBuf::from("./test_cases/extracted-npm/hello.js").exists());
    remove_dir_all(PathBuf::from("./test_cases/extracted-npm/")).unwrap();
  }
}
