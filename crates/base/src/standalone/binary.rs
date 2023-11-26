use crate::js_worker::emitter::EmitterFactory;
use crate::standalone::virtual_fs::{FileBackedVfs, VfsBuilder, VfsRoot, VirtualDirectory};
use crate::standalone::VFS_ESZIP_KEY;
use crate::utils::graph_util::{create_eszip_from_graph_raw, create_graph, ModuleGraphBuilder};
use deno_ast::ModuleSpecifier;
use deno_core::error::AnyError;
use deno_core::serde_json;
use deno_npm::registry::PackageDepNpmSchemeValueParseError;
use deno_npm::NpmSystemInfo;
use deno_semver::package::PackageReq;
use deno_semver::VersionReqSpecifierParseError;
use eszip::EszipV2;
use log::Level;
use module_fetcher::args::package_json::{PackageJsonDepValueParseError, PackageJsonDeps};
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use url::Url;

#[derive(Serialize, Deserialize)]
enum SerializablePackageJsonDepValueParseError {
    SchemeValue(String),
    Specifier(String),
    Unsupported { scheme: String },
}

impl SerializablePackageJsonDepValueParseError {
    pub fn from_err(err: PackageJsonDepValueParseError) -> Self {
        match err {
            PackageJsonDepValueParseError::SchemeValue(err) => Self::SchemeValue(err.value),
            PackageJsonDepValueParseError::Specifier(err) => {
                Self::Specifier(err.source.to_string())
            }
            PackageJsonDepValueParseError::Unsupported { scheme } => Self::Unsupported { scheme },
        }
    }

    pub fn into_err(self) -> PackageJsonDepValueParseError {
        match self {
            SerializablePackageJsonDepValueParseError::SchemeValue(value) => {
                PackageJsonDepValueParseError::SchemeValue(PackageDepNpmSchemeValueParseError {
                    value,
                })
            }
            SerializablePackageJsonDepValueParseError::Specifier(source) => {
                PackageJsonDepValueParseError::Specifier(VersionReqSpecifierParseError {
                    source: monch::ParseErrorFailureError::new(source),
                })
            }
            SerializablePackageJsonDepValueParseError::Unsupported { scheme } => {
                PackageJsonDepValueParseError::Unsupported { scheme }
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct SerializablePackageJsonDeps(
    BTreeMap<String, Result<PackageReq, SerializablePackageJsonDepValueParseError>>,
);

impl SerializablePackageJsonDeps {
    pub fn from_deps(deps: PackageJsonDeps) -> Self {
        Self(
            deps.into_iter()
                .map(|(name, req)| {
                    let res = req.map_err(SerializablePackageJsonDepValueParseError::from_err);
                    (name, res)
                })
                .collect(),
        )
    }

    pub fn into_deps(self) -> PackageJsonDeps {
        self.0
            .into_iter()
            .map(|(name, res)| (name, res.map_err(|err| err.into_err())))
            .collect()
    }
}

#[derive(Deserialize, Serialize)]
pub struct Metadata {
    // pub argv: Vec<String>,
    // pub unstable: bool,
    // pub seed: Option<u64>,
    // pub location: Option<Url>,
    // pub v8_flags: Vec<String>,
    pub ca_stores: Option<Vec<String>>,
    pub ca_data: Option<Vec<u8>>,
    pub unsafely_ignore_certificate_errors: Option<Vec<String>>,
    pub maybe_import_map: Option<(Url, String)>,
    // pub entrypoint: ModuleSpecifier,
    // /// Whether this uses a node_modules directory (true) or the global cache (false).
    // pub node_modules_dir: bool,
    pub package_json_deps: Option<SerializablePackageJsonDeps>,
}

pub fn load_npm_vfs(root_dir_path: PathBuf, vfs_data: &Vec<u8>) -> Result<FileBackedVfs, AnyError> {
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

pub fn build_vfs(emitter_factory: Arc<EmitterFactory>) -> Result<VfsBuilder, AnyError> {
    let npm_resolver = emitter_factory.npm_resolver();
    if let Some(node_modules_path) = npm_resolver.node_modules_path() {
        let mut builder = VfsBuilder::new(node_modules_path.clone())?;
        builder.add_dir_recursive(&node_modules_path)?;
        Ok(builder)
    } else {
        // DO NOT include the user's registry url as it may contain credentials,
        // but also don't make this dependent on the registry url
        let registry_url = emitter_factory.npm_api().base_url();
        let root_path = emitter_factory.npm_cache().registry_folder(registry_url);
        let mut builder = VfsBuilder::new(root_path)?;
        for package in emitter_factory
            .npm_resolution()
            .all_system_packages(&NpmSystemInfo::default())
        {
            let folder = emitter_factory
                .npm_resolver()
                .resolve_pkg_folder_from_pkg_id(&package.id)?;
            builder.add_dir_recursive(&folder)?;
        }
        // overwrite the root directory's name to obscure the user's registry url
        builder.set_root_dir_name("node_modules".to_string());
        Ok(builder)
    }
}

pub async fn generate_binary_eszip(
    file: PathBuf,
    emitter_factory: Arc<EmitterFactory>,
) -> Result<EszipV2, AnyError> {
    let graph = create_graph(file, emitter_factory.clone()).await;
    let mut eszip = create_eszip_from_graph_raw(graph, Some(emitter_factory.clone())).await;

    let npm_res = emitter_factory.npm_resolution();

    let (npm_vfs, npm_files) = if npm_res.has_packages() {
        let (root_dir, files) = build_vfs(emitter_factory.clone())?.into_dir_and_files();
        let snapshot = npm_res.serialized_valid_snapshot_for_system(&NpmSystemInfo::default());
        eszip.add_npm_snapshot(snapshot);
        (Some(root_dir), files)
    } else {
        (None, Vec::new())
    };

    let npm_vfs = serde_json::to_string(&npm_vfs)?.as_bytes().to_vec();
    let boxed_slice = npm_vfs.into_boxed_slice();

    eszip.add_opaque_data(String::from(VFS_ESZIP_KEY), Arc::from(boxed_slice));

    Ok(eszip)
}
