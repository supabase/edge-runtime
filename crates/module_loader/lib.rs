use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use deno::deno_npm::resolution::ValidSerializedNpmResolutionSnapshot;
use deno_core::FastString;
use deno_core::ModuleLoader;
use ext_node::NodeExtInitServices;
use fs::virtual_fs::FileBackedVfs;
use fs::EszipStaticFiles;

pub mod metadata;
pub mod standalone;
pub mod util;

pub struct RuntimeProviders {
  pub node_services: NodeExtInitServices,
  pub module_loader: Rc<dyn ModuleLoader>,
  pub vfs: Arc<FileBackedVfs>,
  pub module_code: Option<FastString>,
  pub static_files: EszipStaticFiles,
  pub npm_snapshot: Option<ValidSerializedNpmResolutionSnapshot>,
  pub vfs_path: PathBuf,
}
