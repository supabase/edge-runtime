use crate::virtual_fs::{FileBackedVfs, VfsBuilder, VfsRoot, VirtualDirectory};
use anyhow::{bail, Context};
use deno_core::normalize_path;
use deno_npm::NpmSystemInfo;
use eszip::EszipV2;
use eszip_async_trait::{AsyncEszipDataRead, STATIC_FILES_ESZIP_KEY};
use indexmap::IndexMap;
use log::warn;
use npm::{CliNpmResolver, InnerCliNpmResolverRef};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use url::Url;
use virtual_fs::VfsEntry;

mod r#impl;
mod rt;

pub use r#impl::deno_compile_fs;
pub use r#impl::prefix_fs;
pub use r#impl::s3_fs;
pub use r#impl::static_fs;
pub use r#impl::tmp_fs;
pub use r#impl::virtual_fs;
pub use rt::IO_RT;

pub struct VfsOpts {
    pub npm_resolver: Arc<dyn CliNpmResolver>,
}

pub type EszipStaticFiles = HashMap<PathBuf, String>;

pub trait LazyEszipV2: std::ops::Deref<Target = EszipV2> + AsyncEszipDataRead {}

impl<T> LazyEszipV2 for T where T: std::ops::Deref<Target = EszipV2> + AsyncEszipDataRead {}

pub async fn extract_static_files_from_eszip<P>(
    eszip: &dyn LazyEszipV2,
    mapped_static_root_path: P,
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
            normalize_path(mapped_static_root_path.as_ref().join(path)),
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
                let root_path = npm_resolver.global_cache_root_folder();
                let mut builder = VfsBuilder::new(root_path, add_content_callback_fn)?;
                let mut packages = npm_resolver.all_system_packages(&NpmSystemInfo::default());
                packages.sort_by(|a, b| a.id.cmp(&b.id)); // determinism
                for package in packages {
                    let folder = npm_resolver.resolve_pkg_folder_from_pkg_id(&package.id)?;
                    builder.add_dir_recursive(&folder)?;
                }

                // Flatten all the registries folders into a single "node_modules/localhost" folder
                // that will be used by denort when loading the npm cache. This avoids us exposing
                // the user's private registry information and means we don't have to bother
                // serializing all the different registry config into the binary.
                builder.with_root_dir(|root_dir| {
                    root_dir.name = "node_modules".to_string();
                    let mut new_entries = Vec::with_capacity(root_dir.entries.len());
                    let mut localhost_entries = IndexMap::new();
                    for entry in std::mem::take(&mut root_dir.entries) {
                        match entry {
                            VfsEntry::Dir(dir) => {
                                for entry in dir.entries {
                                    log::debug!("Flattening {} into node_modules", entry.name());
                                    if let Some(existing) =
                                        localhost_entries.insert(entry.name().to_string(), entry)
                                    {
                                        panic!(
                                            "Unhandled scenario where a duplicate entry was found: {:?}",
                                            existing
                                        );
                                    }
                                }
                            }
                            VfsEntry::File(_) | VfsEntry::Symlink(_) => {
                                new_entries.push(entry);
                            }
                        }
                    }
                    new_entries.push(VfsEntry::Dir(VirtualDirectory {
                        name: "localhost".to_string(),
                        entries: localhost_entries.into_iter().map(|(_, v)| v).collect(),
                    }));
                    // needs to be sorted by name
                    new_entries.sort_by(|a, b| a.name().cmp(b.name()));
                    root_dir.entries = new_entries;
                });

                Ok(builder)
            }
        }

        _ => {
            unreachable!();
        }
    }
}
