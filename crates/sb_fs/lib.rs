use crate::virtual_fs::{FileBackedVfs, VfsBuilder, VfsRoot, VirtualDirectory};
use deno_core::error::AnyError;
use deno_core::serde_json;
use deno_npm::NpmSystemInfo;
use sb_npm::cache::NpmCache;
use sb_npm::registry::CliNpmRegistryApi;
use sb_npm::resolution::NpmResolution;
use sb_npm::{CliNpmResolver, InnerCliNpmResolverRef};
use std::path::PathBuf;
use std::sync::Arc;

pub mod file_system;
pub mod virtual_fs;

pub struct VfsOpts {
    pub npm_resolver: Arc<dyn CliNpmResolver>,
    pub npm_registry_api: Arc<CliNpmRegistryApi>,
    pub npm_cache: Arc<NpmCache>,
    pub npm_resolution: Arc<NpmResolution>,
}

pub fn load_npm_vfs(root_dir_path: PathBuf, vfs_data: &[u8]) -> Result<FileBackedVfs, AnyError> {
    let mut dir: VirtualDirectory = serde_json::from_slice(vfs_data)?;

    // align the name of the directory with the root dir
    dir.name = root_dir_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    let fs_root = VfsRoot {
        dir,
        root_path: root_dir_path,
    };
    Ok(FileBackedVfs::new(fs_root))
}

pub fn build_vfs(opts: VfsOpts) -> Result<VfsBuilder, AnyError> {
    match opts.npm_resolver.as_inner() {
        InnerCliNpmResolverRef::Managed(npm_resolver) => {
            if let Some(node_modules_path) = npm_resolver.root_node_modules_path() {
                let mut builder = VfsBuilder::new(node_modules_path.clone())?;
                builder.add_dir_recursive(node_modules_path)?;
                Ok(builder)
            } else {
                // DO NOT include the user's registry url as it may contain credentials,
                // but also don't make this dependent on the registry url
                let registry_url = npm_resolver.registry_base_url();
                let root_path = npm_resolver.registry_folder_in_global_cache(registry_url);
                let mut builder = VfsBuilder::new(root_path)?;
                for package in npm_resolver.all_system_packages(&NpmSystemInfo::default()) {
                    let folder = npm_resolver.resolve_pkg_folder_from_pkg_id(&package.id)?;
                    builder.add_dir_recursive(&folder)?;
                }
                // overwrite the root directory's name to obscure the user's registry url
                builder.set_root_dir_name("node_modules".to_string());
                Ok(builder)
            }
        }
        _ => {
            panic!("Unreachable");
        }
    }
}
