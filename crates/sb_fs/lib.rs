use crate::virtual_fs::{FileBackedVfs, VfsBuilder, VfsRoot, VirtualDirectory};
use anyhow::{bail, Context};
use deno_core::normalize_path;
use deno_npm::NpmSystemInfo;
use eszip::EszipV2;
use log::warn;
use sb_eszip_shared::{AsyncEszipDataRead, STATIC_FILES_ESZIP_KEY};
use sb_npm::cache::NpmCache;
use sb_npm::registry::CliNpmRegistryApi;
use sb_npm::resolution::NpmResolution;
use sb_npm::{CliNpmResolver, InnerCliNpmResolverRef};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use url::Url;

pub mod file_system;
mod rt;
pub mod static_fs;
pub mod virtual_fs;

pub struct VfsOpts {
    pub npm_resolver: Arc<dyn CliNpmResolver>,
    pub npm_registry_api: Arc<CliNpmRegistryApi>,
    pub npm_cache: Arc<NpmCache>,
    pub npm_resolution: Arc<NpmResolution>,
}

pub type EszipStaticFiles = HashMap<PathBuf, String>;

pub trait LazyEszipV2: std::ops::Deref<Target = EszipV2> + AsyncEszipDataRead {}

impl<T> LazyEszipV2 for T where T: std::ops::Deref<Target = EszipV2> + AsyncEszipDataRead {}

pub async fn extract_static_files_from_eszip<P>(
    eszip: &dyn LazyEszipV2,
    mapped_base_dir_path: P,
) -> EszipStaticFiles
where
    P: AsRef<Path>,
{
    let mut files = EszipStaticFiles::default();

    let Some(eszip_static_files) = eszip.ensure_module(STATIC_FILES_ESZIP_KEY) else {
        return files;
    };

    let data = eszip_static_files.source().await.unwrap();
    let archived = match rkyv::check_archived_root::<Vec<String>>(&data) {
        Ok(vec) => vec,
        Err(err) => {
            warn!("failed to deserialize specifiers for static files: {}", err);
            return files;
        }
    };

    for specifier in archived.as_ref() {
        let specifier = specifier.as_str();
        let path = match Url::parse(specifier) {
            Ok(v) => PathBuf::from(v.path()),
            Err(err) => {
                warn!("could not parse the specifier for static file: {}", err);
                continue;
            }
        };

        files.insert(
            normalize_path(mapped_base_dir_path.as_ref().join(path)),
            specifier.to_string(),
        );
    }

    files
}

pub fn load_npm_vfs(
    eszip: Arc<dyn AsyncEszipDataRead + 'static>,
    root_dir_path: PathBuf,
    vfs_data_slice: Option<&[u8]>,
) -> Result<FileBackedVfs, anyhow::Error> {
    let dir = match vfs_data_slice
        .map(rkyv::check_archived_root::<Option<VirtualDirectory>>)
        .transpose()
    {
        Ok(Some(archived)) => Some(
            <<Option<VirtualDirectory> as rkyv::Archive>::Archived as rkyv::Deserialize<
                Option<VirtualDirectory>,
                rkyv::Infallible,
            >>::deserialize(archived, &mut rkyv::Infallible)
            .with_context(|| "cannot deserialize vfs data")?,
        ),

        Ok(None) => None,
        Err(err) => bail!("cannot load npm vfs: {}", err),
    }
    .flatten();

    let fs_root: VfsRoot = if let Some(mut dir) = dir {
        // align the name of the directory with the root dir
        dir.name = root_dir_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();

        VfsRoot {
            dir,
            root_path: root_dir_path,
        }
    } else {
        VfsRoot {
            dir: VirtualDirectory {
                name: "".to_string(),
                entries: vec![],
            },
            root_path: root_dir_path, // < we should still use the temp, otherwise it might fail when doing `.start_with`
        }
    };

    Ok(FileBackedVfs::new(eszip, fs_root))
}

pub fn build_vfs<'scope, F>(
    opts: VfsOpts,
    add_content_callback_fn: F,
) -> Result<VfsBuilder<'scope>, anyhow::Error>
where
    F: (for<'r> FnMut(&'r Path, &'r str, Vec<u8>) -> String) + 'scope,
{
    match opts.npm_resolver.as_inner() {
        InnerCliNpmResolverRef::Managed(npm_resolver) => {
            if let Some(node_modules_path) = npm_resolver.root_node_modules_path() {
                let mut builder =
                    VfsBuilder::new(node_modules_path.clone(), add_content_callback_fn)?;

                builder.add_dir_recursive(node_modules_path)?;
                Ok(builder)
            } else {
                // DO NOT include the user's registry url as it may contain credentials,
                // but also don't make this dependent on the registry url
                let registry_url = npm_resolver.registry_base_url();
                let root_path = npm_resolver.registry_folder_in_global_cache(registry_url);
                let mut builder = VfsBuilder::new(root_path, add_content_callback_fn)?;
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
            unreachable!();
        }
    }
}
