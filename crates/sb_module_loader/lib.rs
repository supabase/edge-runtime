use deno_core::ModuleLoader;
use sb_npm::CliNpmResolver;
use std::rc::Rc;
use std::sync::Arc;

pub mod metadata;
pub mod node;
pub mod standalone;
pub mod util;

pub struct RuntimeProviders {
    pub npm_resolver: Arc<CliNpmResolver>,
    pub module_loader: Rc<dyn ModuleLoader>,
    pub fs: Arc<dyn deno_fs::FileSystem>,
}
