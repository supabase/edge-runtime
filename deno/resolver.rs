use std::borrow::Cow;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use deno_fs::FileSystem;
use ext_node::DenoFsNodeResolverEnv;

pub type CjsTracker = deno_resolver::cjs::CjsTracker<DenoFsNodeResolverEnv>;

#[derive(Debug, Clone)]
pub struct CliDenoResolverFs(pub Arc<dyn FileSystem>);

impl deno_resolver::fs::DenoResolverFs for CliDenoResolverFs {
  fn read_to_string_lossy(
    &self,
    path: &Path,
  ) -> std::io::Result<Cow<'static, str>> {
    self
      .0
      .read_text_file_lossy_sync(path, None)
      .map_err(|e| e.into_io_error())
  }

  fn realpath_sync(&self, path: &Path) -> std::io::Result<PathBuf> {
    self.0.realpath_sync(path).map_err(|e| e.into_io_error())
  }

  fn exists_sync(&self, path: &Path) -> bool {
    self.0.exists_sync(path)
  }

  fn is_dir_sync(&self, path: &Path) -> bool {
    self.0.is_dir_sync(path)
  }

  fn read_dir_sync(
    &self,
    dir_path: &Path,
  ) -> std::io::Result<Vec<deno_resolver::fs::DirEntry>> {
    self
      .0
      .read_dir_sync(dir_path)
      .map(|entries| {
        entries
          .into_iter()
          .map(|e| deno_resolver::fs::DirEntry {
            name: e.name,
            is_file: e.is_file,
            is_directory: e.is_directory,
          })
          .collect::<Vec<_>>()
      })
      .map_err(|err| err.into_io_error())
  }
}
