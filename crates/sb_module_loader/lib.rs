use deno_core::{FastString, ModuleLoader};
use deno_npm::resolution::ValidSerializedNpmResolutionSnapshot;
use sb_fs::virtual_fs::FileBackedVfs;
use sb_fs::EszipStaticFiles;
use sb_node::NpmResolver;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

pub mod metadata;
pub mod node;
pub mod standalone;
pub mod util;

pub struct RuntimeProviders {
    pub npm_resolver: Arc<dyn NpmResolver>,
    pub module_loader: Rc<dyn ModuleLoader>,
    pub vfs: Arc<FileBackedVfs>,
    pub module_code: Option<FastString>,
    pub static_files: EszipStaticFiles,
    pub npm_snapshot: Option<ValidSerializedNpmResolutionSnapshot>,
    pub vfs_path: PathBuf,
}
