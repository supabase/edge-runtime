// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

//! Code for local node_modules resolution.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use crate::cache::mixed_case_package_name_decode;
use async_trait::async_trait;
use deno_ast::ModuleSpecifier;
use deno_core::anyhow::bail;
use deno_core::anyhow::Context;
use deno_core::error::AnyError;
use deno_core::futures;
use deno_core::unsync::spawn;
use deno_core::unsync::JoinHandle;
use deno_core::url::Url;
use deno_fs;
use deno_npm::resolution::NpmResolutionSnapshot;
use deno_npm::NpmPackageCacheFolderId;
use deno_npm::NpmPackageId;
use deno_npm::NpmResolutionPackage;
use deno_npm::NpmSystemInfo;
use deno_semver::package::PackageNv;
use sb_node::NodePermissions;
use sb_node::NodeResolutionMode;
use sb_node::PackageJson;
use serde::Deserialize;
use serde::Serialize;
use sb_core::cache::CACHE_PERM;
use sb_core::util::fs::{atomic_write_file, canonicalize_path_maybe_not_exists_with_fs, copy_dir_recursive, hard_link_dir_recursive, LaxSingleProcessFsFlag, symlink_dir};

use crate::cache::mixed_case_package_name_encode;
use crate::resolution::NpmResolution;
use crate::NpmCache;

use super::common::types_package_name;
use super::common::NpmPackageFsResolver;
use super::common::RegistryReadPermissionChecker;

/// Resolver that creates a local node_modules directory
/// and resolves packages from it.
#[derive(Debug)]
pub struct LocalNpmPackageResolver {
    fs: Arc<dyn deno_fs::FileSystem>,
    cache: Arc<NpmCache>,
    resolution: Arc<NpmResolution>,
    registry_url: Url,
    root_node_modules_path: PathBuf,
    root_node_modules_url: Url,
    system_info: NpmSystemInfo,
    registry_read_permission_checker: RegistryReadPermissionChecker,
}

impl LocalNpmPackageResolver {
    pub fn new(
        fs: Arc<dyn deno_fs::FileSystem>,
        cache: Arc<NpmCache>,
        registry_url: Url,
        node_modules_folder: PathBuf,
        resolution: Arc<NpmResolution>,
        system_info: NpmSystemInfo,
    ) -> Self {
        Self {
            fs: fs.clone(),
            cache,
            resolution,
            registry_url,
            root_node_modules_url: Url::from_directory_path(&node_modules_folder).unwrap(),
            root_node_modules_path: node_modules_folder.clone(),
            system_info,
            registry_read_permission_checker: RegistryReadPermissionChecker::new(
                fs,
                node_modules_folder,
            ),
        }
    }

    fn resolve_package_root(&self, path: &Path) -> PathBuf {
        let mut last_found = path;
        loop {
            let parent = last_found.parent().unwrap();
            if parent.file_name().unwrap() == "node_modules" {
                return last_found.to_path_buf();
            } else {
                last_found = parent;
            }
        }
    }

    fn resolve_folder_for_specifier(
        &self,
        specifier: &ModuleSpecifier,
    ) -> Result<Option<PathBuf>, AnyError> {
        let Some(relative_url) = self.root_node_modules_url.make_relative(specifier) else {
            return Ok(None);
        };
        if relative_url.starts_with("../") {
            return Ok(None);
        }
        // it's within the directory, so use it
        let Some(path) = specifier.to_file_path().ok() else {
            return Ok(None);
        };
        // Canonicalize the path so it's not pointing to the symlinked directory
        // in `node_modules` directory of the referrer.
        canonicalize_path_maybe_not_exists_with_fs(&path, |path| {
            self.fs
                .realpath_sync(path)
                .map_err(|err| err.into_io_error())
        })
            .map(Some)
            .map_err(|err| err.into())
    }
}

#[async_trait]
impl NpmPackageFsResolver for LocalNpmPackageResolver {
    fn root_dir_url(&self) -> &Url {
        &self.root_node_modules_url
    }

    fn node_modules_path(&self) -> Option<PathBuf> {
        Some(self.root_node_modules_path.clone())
    }

    fn package_folder(&self, id: &NpmPackageId) -> Result<PathBuf, AnyError> {
        match self.resolution.resolve_pkg_cache_folder_id_from_pkg_id(id) {
            // package is stored at:
            // node_modules/.deno/<package_cache_folder_id_folder_name>/node_modules/<package_name>
            Some(cache_folder_id) => Ok(self
                .root_node_modules_path
                .join(".deno")
                .join(get_package_folder_id_folder_name(&cache_folder_id))
                .join("node_modules")
                .join(&cache_folder_id.nv.name)),
            None => bail!(
                "Could not find package information for '{}'",
                id.as_serialized()
            ),
        }
    }

    fn resolve_package_folder_from_package(
        &self,
        name: &str,
        referrer: &ModuleSpecifier,
        mode: NodeResolutionMode,
    ) -> Result<PathBuf, AnyError> {
        let Some(local_path) = self.resolve_folder_for_specifier(referrer)? else {
            bail!("could not find npm package for '{}'", referrer);
        };
        let package_root_path = self.resolve_package_root(&local_path);
        let mut current_folder = package_root_path.as_path();
        loop {
            current_folder = current_folder.parent().unwrap();
            let node_modules_folder = if current_folder.ends_with("node_modules") {
                Cow::Borrowed(current_folder)
            } else {
                Cow::Owned(current_folder.join("node_modules"))
            };
            let sub_dir = join_package_name(&node_modules_folder, name);
            if self.fs.is_dir_sync(&sub_dir) {
                // if doing types resolution, only resolve the package if it specifies a types property
                if mode.is_types() && !name.starts_with("@types/") {
                    let package_json = PackageJson::load_skip_read_permission(
                        &*self.fs,
                        sub_dir.join("package.json"),
                    )?;
                    if package_json.types.is_some() {
                        return Ok(sub_dir);
                    }
                } else {
                    return Ok(sub_dir);
                }
            }

            // if doing type resolution, check for the existence of a @types package
            if mode.is_types() && !name.starts_with("@types/") {
                let sub_dir = join_package_name(&node_modules_folder, &types_package_name(name));
                if self.fs.is_dir_sync(&sub_dir) {
                    return Ok(sub_dir);
                }
            }

            if current_folder == self.root_node_modules_path {
                bail!(
                    "could not find package '{}' from referrer '{}'.",
                    name,
                    referrer
                );
            }
        }
    }

    fn resolve_package_folder_from_specifier(
        &self,
        specifier: &ModuleSpecifier,
    ) -> Result<Option<PathBuf>, AnyError> {
        let Some(local_path) = self.resolve_folder_for_specifier(specifier)? else {
            return Ok(None);
        };
        let package_root_path = self.resolve_package_root(&local_path);
        Ok(Some(package_root_path))
    }

    fn resolve_package_cache_folder_id_from_specifier(
        &self,
        specifier: &ModuleSpecifier,
    ) -> Result<Option<NpmPackageCacheFolderId>, AnyError> {
        let Some(folder_path) = self.resolve_package_folder_from_specifier(specifier)? else {
            return Ok(None);
        };
        let folder_name = folder_path.parent().unwrap().to_string_lossy();
        Ok(get_package_folder_id_from_folder_name(&folder_name))
    }

    async fn cache_packages(&self) -> Result<(), AnyError> {
        sync_resolution_with_fs(
            &self.resolution.snapshot(),
            &self.cache,
            &self.registry_url,
            &self.root_node_modules_path,
            &self.system_info,
        )
            .await
    }

    fn ensure_read_permission(
        &self,
        permissions: &dyn NodePermissions,
        path: &Path,
    ) -> Result<(), AnyError> {
        self.registry_read_permission_checker
            .ensure_registry_read_permission(permissions, path)
    }
}

/// Creates a pnpm style folder structure.
async fn sync_resolution_with_fs(
    snapshot: &NpmResolutionSnapshot,
    cache: &Arc<NpmCache>,
    registry_url: &Url,
    root_node_modules_dir_path: &Path,
    system_info: &NpmSystemInfo,
) -> Result<(), AnyError> {
    if snapshot.is_empty() {
        return Ok(()); // don't create the directory
    }

    let deno_local_registry_dir = root_node_modules_dir_path.join(".deno");
    let deno_node_modules_dir = deno_local_registry_dir.join("node_modules");
    fs::create_dir_all(&deno_node_modules_dir)
        .with_context(|| format!("Creating '{}'", deno_local_registry_dir.display()))?;

    let single_process_lock = LaxSingleProcessFsFlag::lock(
        deno_local_registry_dir.join(".deno.lock"),
        // similar message used by cargo build
        "waiting for file lock on node_modules directory",
    )
        .await;

    // load this after we get the directory lock
    let mut setup_cache = SetupCache::load(deno_local_registry_dir.join(".setup-cache.bin"));

    // 1. Write all the packages out the .deno directory.
    //
    // Copy (hardlink in future) <global_registry_cache>/<package_id>/ to
    // node_modules/.deno/<package_folder_id_folder_name>/node_modules/<package_name>
    let package_partitions = snapshot.all_system_packages_partitioned(system_info);
    let mut handles: Vec<JoinHandle<Result<(), AnyError>>> =
        Vec::with_capacity(package_partitions.packages.len());
    let mut newest_packages_by_name: HashMap<&String, &NpmResolutionPackage> =
        HashMap::with_capacity(package_partitions.packages.len());
    for package in &package_partitions.packages {
        if let Some(current_pkg) = newest_packages_by_name.get_mut(&package.id.nv.name) {
            if current_pkg.id.nv.cmp(&package.id.nv) == Ordering::Less {
                *current_pkg = package;
            }
        } else {
            newest_packages_by_name.insert(&package.id.nv.name, package);
        };

        let package_folder_name =
            get_package_folder_id_folder_name(&package.get_package_cache_folder_id());
        let folder_path = deno_local_registry_dir.join(&package_folder_name);
        let initialized_file = folder_path.join(".initialized");
        if !cache
            .cache_setting()
            .should_use_for_npm_package(&package.id.nv.name)
            || !initialized_file.exists()
        {
            // cache bust the dep from the dep setup cache so the symlinks
            // are forced to be recreated
            setup_cache.remove_dep(&package_folder_name);

            let cache = cache.clone();
            let registry_url = registry_url.clone();
            let package = package.clone();
            let handle = spawn(async move {
                cache
                    .ensure_package(&package.id.nv, &package.dist, &registry_url)
                    .await?;
                let sub_node_modules = folder_path.join("node_modules");
                let package_path = join_package_name(&sub_node_modules, &package.id.nv.name);
                fs::create_dir_all(&package_path)
                    .with_context(|| format!("Creating '{}'", folder_path.display()))?;
                let cache_folder =
                    cache.package_folder_for_name_and_version(&package.id.nv, &registry_url);
                // for now copy, but in the future consider hard linking
                copy_dir_recursive(&cache_folder, &package_path)?;
                // write out a file that indicates this folder has been initialized
                fs::write(initialized_file, "")?;
                // finally stop showing the progress bar
                Ok(())
            });
            handles.push(handle);
        }
    }

    let results = futures::future::join_all(handles).await;
    for result in results {
        result??; // surface the first error
    }

    // 2. Create any "copy" packages, which are used for peer dependencies
    for package in &package_partitions.copy_packages {
        let package_cache_folder_id = package.get_package_cache_folder_id();
        let destination_path = deno_local_registry_dir
            .join(get_package_folder_id_folder_name(&package_cache_folder_id));
        let initialized_file = destination_path.join(".initialized");
        if !initialized_file.exists() {
            let sub_node_modules = destination_path.join("node_modules");
            let package_path = join_package_name(&sub_node_modules, &package.id.nv.name);
            fs::create_dir_all(&package_path)
                .with_context(|| format!("Creating '{}'", destination_path.display()))?;
            let source_path = join_package_name(
                &deno_local_registry_dir
                    .join(get_package_folder_id_folder_name(
                        &package_cache_folder_id.with_no_count(),
                    ))
                    .join("node_modules"),
                &package.id.nv.name,
            );
            hard_link_dir_recursive(&source_path, &package_path)?;
            // write out a file that indicates this folder has been initialized
            fs::write(initialized_file, "")?;
        }
    }

    // 3. Symlink all the dependencies into the .deno directory.
    //
    // Symlink node_modules/.deno/<package_id>/node_modules/<dep_name> to
    // node_modules/.deno/<dep_id>/node_modules/<dep_package_name>
    for package in package_partitions.iter_all() {
        let package_folder_name =
            get_package_folder_id_folder_name(&package.get_package_cache_folder_id());
        let sub_node_modules = deno_local_registry_dir
            .join(&package_folder_name)
            .join("node_modules");
        let mut dep_setup_cache = setup_cache.with_dep(&package_folder_name);
        for (name, dep_id) in &package.dependencies {
            let dep_cache_folder_id = snapshot
                .package_from_id(dep_id)
                .unwrap()
                .get_package_cache_folder_id();
            let dep_folder_name = get_package_folder_id_folder_name(&dep_cache_folder_id);
            if dep_setup_cache.insert(name, &dep_folder_name) {
                let dep_folder_path = join_package_name(
                    &deno_local_registry_dir
                        .join(dep_folder_name)
                        .join("node_modules"),
                    &dep_id.nv.name,
                );
                symlink_package_dir(
                    &dep_folder_path,
                    &join_package_name(&sub_node_modules, name),
                )?;
            }
        }
    }

    // 4. Create all the top level packages in the node_modules folder, which are symlinks.
    //
    // Symlink node_modules/<package_name> to
    // node_modules/.deno/<package_id>/node_modules/<package_name>
    let mut found_names = HashSet::new();
    let mut ids = snapshot.top_level_packages().collect::<Vec<_>>();
    ids.sort_by(|a, b| b.cmp(a)); // create determinism and only include the latest version
    for id in ids {
        if !found_names.insert(&id.nv.name) {
            continue; // skip, already handled
        }
        let package = snapshot.package_from_id(id).unwrap();
        let target_folder_name =
            get_package_folder_id_folder_name(&package.get_package_cache_folder_id());
        if setup_cache.insert_root_symlink(&id.nv.name, &target_folder_name) {
            let local_registry_package_path = join_package_name(
                &deno_local_registry_dir
                    .join(target_folder_name)
                    .join("node_modules"),
                &id.nv.name,
            );

            symlink_package_dir(
                &local_registry_package_path,
                &join_package_name(root_node_modules_dir_path, &id.nv.name),
            )?;
        }
    }

    // 5. Create a node_modules/.deno/node_modules/<package-name> directory with
    // the remaining packages
    for package in newest_packages_by_name.values() {
        if !found_names.insert(&package.id.nv.name) {
            continue; // skip, already handled
        }

        let target_folder_name =
            get_package_folder_id_folder_name(&package.get_package_cache_folder_id());
        if setup_cache.insert_deno_symlink(&package.id.nv.name, &target_folder_name) {
            let local_registry_package_path = join_package_name(
                &deno_local_registry_dir
                    .join(target_folder_name)
                    .join("node_modules"),
                &package.id.nv.name,
            );

            symlink_package_dir(
                &local_registry_package_path,
                &join_package_name(&deno_node_modules_dir, &package.id.nv.name),
            )?;
        }
    }

    setup_cache.save();
    drop(single_process_lock);

    Ok(())
}

/// Represents a dependency at `node_modules/.deno/<package_id>/`
struct SetupCacheDep<'a> {
    previous: Option<&'a HashMap<String, String>>,
    current: &'a mut HashMap<String, String>,
}

impl<'a> SetupCacheDep<'a> {
    pub fn insert(&mut self, name: &str, target_folder_name: &str) -> bool {
        self.current
            .insert(name.to_string(), target_folder_name.to_string());
        if let Some(previous_target) = self.previous.and_then(|p| p.get(name)) {
            previous_target != target_folder_name
        } else {
            true
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
struct SetupCacheData {
    root_symlinks: HashMap<String, String>,
    deno_symlinks: HashMap<String, String>,
    dep_symlinks: HashMap<String, HashMap<String, String>>,
}

/// It is very slow to try to re-setup the symlinks each time, so this will
/// cache what we've setup on the last run and only update what is necessary.
/// Obviously this could lead to issues if the cache gets out of date with the
/// file system, such as if the user manually deletes a symlink.
struct SetupCache {
    file_path: PathBuf,
    previous: Option<SetupCacheData>,
    current: SetupCacheData,
}

impl SetupCache {
    pub fn load(file_path: PathBuf) -> Self {
        let previous = std::fs::read(&file_path)
            .ok()
            .and_then(|data| bincode::deserialize(&data).ok());
        Self {
            file_path,
            previous,
            current: Default::default(),
        }
    }

    pub fn save(&self) -> bool {
        if let Some(previous) = &self.previous {
            if previous == &self.current {
                return false; // nothing to save
            }
        }

        bincode::serialize(&self.current)
            .ok()
            .and_then(|data| atomic_write_file(&self.file_path, data, CACHE_PERM).ok());
        true
    }

    /// Inserts and checks for the existence of a root symlink
    /// at `node_modules/<package_name>` pointing to
    /// `node_modules/.deno/<package_id>/`
    pub fn insert_root_symlink(&mut self, name: &str, target_folder_name: &str) -> bool {
        self.current
            .root_symlinks
            .insert(name.to_string(), target_folder_name.to_string());
        if let Some(previous_target) = self
            .previous
            .as_ref()
            .and_then(|p| p.root_symlinks.get(name))
        {
            previous_target != target_folder_name
        } else {
            true
        }
    }

    /// Inserts and checks for the existence of a symlink at
    /// `node_modules/.deno/node_modules/<package_name>` pointing to
    /// `node_modules/.deno/<package_id>/`
    pub fn insert_deno_symlink(&mut self, name: &str, target_folder_name: &str) -> bool {
        self.current
            .deno_symlinks
            .insert(name.to_string(), target_folder_name.to_string());
        if let Some(previous_target) = self
            .previous
            .as_ref()
            .and_then(|p| p.deno_symlinks.get(name))
        {
            previous_target != target_folder_name
        } else {
            true
        }
    }

    pub fn remove_dep(&mut self, parent_name: &str) {
        if let Some(previous) = &mut self.previous {
            previous.dep_symlinks.remove(parent_name);
        }
    }

    pub fn with_dep(&mut self, parent_name: &str) -> SetupCacheDep<'_> {
        SetupCacheDep {
            previous: self
                .previous
                .as_ref()
                .and_then(|p| p.dep_symlinks.get(parent_name)),
            current: self
                .current
                .dep_symlinks
                .entry(parent_name.to_string())
                .or_default(),
        }
    }
}

fn get_package_folder_id_folder_name(folder_id: &NpmPackageCacheFolderId) -> String {
    let copy_str = if folder_id.copy_index == 0 {
        "".to_string()
    } else {
        format!("_{}", folder_id.copy_index)
    };
    let nv = &folder_id.nv;
    let name = if nv.name.to_lowercase() == nv.name {
        Cow::Borrowed(&nv.name)
    } else {
        Cow::Owned(format!("_{}", mixed_case_package_name_encode(&nv.name)))
    };
    format!("{}@{}{}", name, nv.version, copy_str).replace('/', "+")
}

fn get_package_folder_id_from_folder_name(folder_name: &str) -> Option<NpmPackageCacheFolderId> {
    let folder_name = folder_name.replace('+', "/");
    let (name, ending) = folder_name.rsplit_once('@')?;
    let name = if let Some(encoded_name) = name.strip_prefix('_') {
        mixed_case_package_name_decode(encoded_name)?
    } else {
        name.to_string()
    };
    let (raw_version, copy_index) = match ending.split_once('_') {
        Some((raw_version, copy_index)) => {
            let copy_index = copy_index.parse::<u8>().ok()?;
            (raw_version, copy_index)
        }
        None => (ending, 0),
    };
    let version = deno_semver::Version::parse_from_npm(raw_version).ok()?;
    Some(NpmPackageCacheFolderId {
        nv: PackageNv { name, version },
        copy_index,
    })
}

fn symlink_package_dir(old_path: &Path, new_path: &Path) -> Result<(), AnyError> {
    let new_parent = new_path.parent().unwrap();
    if new_parent.file_name().unwrap() != "node_modules" {
        // create the parent folder that will contain the symlink
        fs::create_dir_all(new_parent)
            .with_context(|| format!("Creating '{}'", new_parent.display()))?;
    }

    // need to delete the previous symlink before creating a new one
    let _ignore = fs::remove_dir_all(new_path);

    #[cfg(windows)]
    return junction_or_symlink_dir(old_path, new_path);
    #[cfg(not(windows))]
    symlink_dir(old_path, new_path)
}

#[cfg(windows)]
fn junction_or_symlink_dir(old_path: &Path, new_path: &Path) -> Result<(), AnyError> {
    // Use junctions because they're supported on ntfs file systems without
    // needing to elevate privileges on Windows

    match junction::create(old_path, new_path) {
        Ok(()) => Ok(()),
        Err(junction_err) => {
            if cfg!(debug) {
                // When running the tests, junctions should be created, but if not then
                // surface this error.
                println!("Error creating junction. {:#}", junction_err);
            }

            match symlink_dir(old_path, new_path) {
                Ok(()) => Ok(()),
                Err(symlink_err) => bail!(
                    concat!(
                        "Failed creating junction and fallback symlink in node_modules folder.\n\n",
                        "{:#}\n\n{:#}",
                    ),
                    junction_err,
                    symlink_err,
                ),
            }
        }
    }
}

fn join_package_name(path: &Path, package_name: &str) -> PathBuf {
    let mut path = path.to_path_buf();
    // ensure backslashes are used on windows
    for part in package_name.split('/') {
        path = path.join(part);
    }
    path
}
