use crate::metadata::Metadata;
use crate::node::cjs_code_anaylzer::CliCjsCodeAnalyzer;
use crate::node::cli_node_resolver::CliNodeResolver;
use crate::node::node_module_loader::{CjsResolutionStore, NpmModuleLoader};
use crate::standalone::standalone_module_loader::{EmbeddedModuleLoader, SharedModuleLoaderState};
use crate::RuntimeProviders;
use anyhow::Context;
use deno_core::error::AnyError;
use deno_core::url::Url;
use deno_core::{FastString, ModuleSpecifier};
use deno_tls::rustls::RootCertStore;
use deno_tls::RootCertStoreProvider;
use import_map::{parse_from_json, ImportMap};
use sb_core::cache::caches::Caches;
use sb_core::cache::deno_dir::DenoDirProvider;
use sb_core::cache::node::NodeAnalysisCache;
use sb_core::cache::CacheSetting;
use sb_core::cert::{get_root_cert_store, CaData};
use sb_core::util::http_util::HttpClient;
use sb_fs::file_system::DenoCompileFileSystem;
use sb_fs::load_npm_vfs;
use sb_graph::graph_resolver::MappedSpecifierResolver;
use sb_graph::{payload_to_eszip, EszipPayloadKind, SOURCE_CODE_ESZIP_KEY, VFS_ESZIP_KEY};
use sb_node::analyze::NodeCodeTranslator;
use sb_node::NodeResolver;
use sb_npm::cache_dir::NpmCacheDir;
use sb_npm::package_json::PackageJsonDepsProvider;
use sb_npm::{
    create_managed_npm_resolver, CliNpmResolverManagedCreateOptions,
    CliNpmResolverManagedPackageJsonInstallerOption, CliNpmResolverManagedSnapshotOption,
};
use std::rc::Rc;
use std::sync::Arc;

pub mod standalone_module_loader;

pub struct StandaloneModuleLoaderFactory {
    shared: Arc<SharedModuleLoaderState>,
}

struct StandaloneRootCertStoreProvider {
    ca_stores: Option<Vec<String>>,
    ca_data: Option<CaData>,
    cell: once_cell::sync::OnceCell<RootCertStore>,
}

impl RootCertStoreProvider for StandaloneRootCertStoreProvider {
    fn get_or_try_init(&self) -> Result<&RootCertStore, AnyError> {
        self.cell.get_or_try_init(|| {
            get_root_cert_store(None, self.ca_stores.clone(), self.ca_data.clone())
                .map_err(|err| err.into())
        })
    }
}

pub async fn create_module_loader_for_eszip(
    mut eszip: eszip::EszipV2,
    metadata: Metadata,
    maybe_import_map: Option<ImportMap>,
) -> Result<RuntimeProviders, AnyError> {
    // let main_module = &metadata.entrypoint;
    let current_exe_path = std::env::current_exe().unwrap();
    let current_exe_name = current_exe_path.file_name().unwrap().to_string_lossy();
    let deno_dir_provider = Arc::new(DenoDirProvider::new(None));
    let root_cert_store_provider = Arc::new(StandaloneRootCertStoreProvider {
        ca_stores: metadata.ca_stores,
        ca_data: metadata.ca_data.map(CaData::Bytes),
        cell: Default::default(),
    });
    let http_client = Arc::new(HttpClient::new(
        Some(root_cert_store_provider.clone()),
        metadata.unsafely_ignore_certificate_errors.clone(),
    ));

    // use a dummy npm registry url
    let npm_registry_url = ModuleSpecifier::parse("https://localhost/").unwrap();
    let root_path = std::env::temp_dir()
        .join(format!("sb-compile-{}", current_exe_name))
        .join("node_modules");
    let npm_cache_dir = NpmCacheDir::new(root_path.clone());
    let npm_global_cache_dir = npm_cache_dir.get_cache_location();

    let code_fs = if let Some(module) = eszip.get_module(SOURCE_CODE_ESZIP_KEY) {
        if let Some(code) = module.take_source().await {
            Some(FastString::from(String::from_utf8(code.to_vec())?))
        } else {
            None
        }
    } else {
        None
    };

    let (fs, snapshot) = if let Some(snapshot) = eszip.take_npm_snapshot() {
        // TODO: Support node_modules
        let vfs_root_dir_path = npm_cache_dir.registry_folder(&npm_registry_url);

        let vfs_data: Vec<u8> = eszip
            .get_module(VFS_ESZIP_KEY)
            .unwrap()
            .take_source()
            .await
            .unwrap()
            .to_vec();

        let vfs = load_npm_vfs(vfs_root_dir_path, &vfs_data).context("Failed to load npm vfs.")?;

        (
            Arc::new(DenoCompileFileSystem::new(vfs)) as Arc<dyn deno_fs::FileSystem>,
            Some(snapshot),
        )
    } else {
        (
            Arc::new(deno_fs::RealFs) as Arc<dyn deno_fs::FileSystem>,
            None,
        )
    };

    let package_json_deps_provider = Arc::new(PackageJsonDepsProvider::new(
        metadata
            .package_json_deps
            .map(|serialized| serialized.into_deps()),
    ));

    let npm_resolver = create_managed_npm_resolver(CliNpmResolverManagedCreateOptions {
        snapshot: CliNpmResolverManagedSnapshotOption::Specified(snapshot),
        maybe_lockfile: None,
        fs: fs.clone(),
        http_client,
        npm_global_cache_dir,
        cache_setting: CacheSetting::Use,
        maybe_node_modules_path: None,
        npm_system_info: Default::default(),
        package_json_installer: CliNpmResolverManagedPackageJsonInstallerOption::ConditionalInstall(
            package_json_deps_provider.clone(),
        ),
        npm_registry_url,
    })
    .await?;

    let node_resolver = Arc::new(NodeResolver::new(
        fs.clone(),
        npm_resolver.clone().into_npm_resolver(),
    ));
    let cjs_resolutions = Arc::new(CjsResolutionStore::default());
    let cache_db = Caches::new(deno_dir_provider.clone());
    let node_analysis_cache = NodeAnalysisCache::new(cache_db.node_analysis_db());
    let cjs_esm_code_analyzer = CliCjsCodeAnalyzer::new(node_analysis_cache, fs.clone());
    let node_code_translator = Arc::new(NodeCodeTranslator::new(
        cjs_esm_code_analyzer,
        fs.clone(),
        node_resolver.clone(),
        npm_resolver.clone().into_npm_resolver(),
    ));
    let maybe_import_map = maybe_import_map
        .map(|import_map| Some(Arc::new(import_map)))
        .unwrap_or_else(|| None);

    let cli_node_resolver = Arc::new(CliNodeResolver::new(
        cjs_resolutions.clone(),
        node_resolver.clone(),
        npm_resolver.clone(),
    ));

    let module_loader_factory = StandaloneModuleLoaderFactory {
        shared: Arc::new(SharedModuleLoaderState {
            eszip,
            mapped_specifier_resolver: MappedSpecifierResolver::new(
                maybe_import_map,
                package_json_deps_provider.clone(),
            ),
            node_resolver: cli_node_resolver.clone(),
            npm_module_loader: Arc::new(NpmModuleLoader::new(
                cjs_resolutions,
                node_code_translator,
                fs.clone(),
                cli_node_resolver,
            )),
        }),
    };

    Ok(RuntimeProviders {
        module_loader: Rc::new(EmbeddedModuleLoader {
            shared: module_loader_factory.shared.clone(),
        }),
        npm_resolver: npm_resolver.into_npm_resolver(),
        fs,
        module_code: code_fs,
    })
}

pub async fn create_module_loader_for_standalone_from_eszip_kind(
    eszip_payload_kind: EszipPayloadKind,
    maybe_import_map_arc: Option<Arc<ImportMap>>,
    maybe_import_map_path: Option<String>,
) -> Result<RuntimeProviders, AnyError> {
    let eszip = payload_to_eszip(eszip_payload_kind).await;

    let mut maybe_import_map: Option<ImportMap> = None;

    if let Some(import_map) = maybe_import_map_arc {
        let clone_import_map = (*import_map).clone();
        maybe_import_map = Some(clone_import_map);
    } else if let Some(import_map_path) = maybe_import_map_path {
        let import_map_url = Url::parse(import_map_path.as_str())?;
        if let Some(import_map_module) = eszip.get_import_map(import_map_url.as_str()) {
            if let Some(source) = import_map_module.source().await {
                let source = std::str::from_utf8(&source)?.to_string();
                let result = parse_from_json(&import_map_url, &source)?;
                maybe_import_map = Some(result.import_map);
            }
        }
    }

    create_module_loader_for_eszip(
        eszip,
        Metadata {
            ca_stores: None,
            ca_data: None,
            unsafely_ignore_certificate_errors: None,
            package_json_deps: None,
        },
        maybe_import_map,
    )
    .await
}
