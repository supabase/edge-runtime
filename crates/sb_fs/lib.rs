use crate::virtual_fs::{FileBackedVfs, VfsBuilder, VfsRoot, VirtualDirectory};
use deno_core::error::AnyError;
use deno_core::{normalize_path, serde_json};
use deno_npm::NpmSystemInfo;
use eszip::EszipV2;
use indexmap::IndexMap;
use sb_npm::{CliNpmResolver, InnerCliNpmResolverRef};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use virtual_fs::VfsEntry;

pub mod file_system;
pub mod static_fs;
pub mod virtual_fs;

pub struct VfsOpts {
    pub npm_resolver: Arc<dyn CliNpmResolver>,
}

pub type EszipStaticFiles = HashMap<String, Vec<u8>>;

pub async fn extract_static_files_from_eszip(eszip: &EszipV2) -> EszipStaticFiles {
    let key = String::from("---SUPABASE-STATIC-FILES-ESZIP---");
    let mut files: EszipStaticFiles = HashMap::new();

    if eszip.specifiers().contains(&key) {
        let eszip_static_files = eszip.get_module(key.as_str()).unwrap();
        let data = eszip_static_files.take_source().await.unwrap();
        let data = data.to_vec();
        let data: Vec<String> = serde_json::from_slice(data.as_slice()).unwrap();
        for static_specifier in data {
            let file_mod = eszip.get_module(static_specifier.as_str()).unwrap();
            files.insert(
                normalize_path(PathBuf::from(static_specifier))
                    .to_str()
                    .unwrap()
                    .to_string(),
                file_mod.take_source().await.unwrap().to_vec(),
            );
        }
    }

    files
}

pub fn load_npm_vfs(
    root_dir_path: PathBuf,
    vfs_data: Option<&[u8]>,
) -> Result<FileBackedVfs, AnyError> {
    let dir: Option<VirtualDirectory> = if let Some(vfs_data) = vfs_data {
        serde_json::from_slice(vfs_data)?
    } else {
        None
    };

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
                let root_path = npm_resolver.global_cache_root_folder();
                let mut builder = VfsBuilder::new(root_path)?;
                for package in npm_resolver.all_system_packages(&NpmSystemInfo::default()) {
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
