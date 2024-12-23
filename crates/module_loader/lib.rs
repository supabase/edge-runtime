use deno_core::{FastString, ModuleLoader};
use deno_npm::resolution::ValidSerializedNpmResolutionSnapshot;
use fs::virtual_fs::FileBackedVfs;
use fs::EszipStaticFiles;
use sb_node::{NodeResolver, NpmResolver};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

pub mod metadata;
pub mod standalone;
pub mod util;

pub struct RuntimeProviders {
    pub node_resolver: Arc<NodeResolver>,
    pub npm_resolver: Arc<dyn NpmResolver>,
    pub module_loader: Rc<dyn ModuleLoader>,
    pub vfs: Arc<FileBackedVfs>,
    pub module_code: Option<FastString>,
    pub static_files: EszipStaticFiles,
    pub npm_snapshot: Option<ValidSerializedNpmResolutionSnapshot>,
    pub vfs_path: PathBuf,
}
