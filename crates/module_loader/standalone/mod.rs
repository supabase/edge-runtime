use crate::metadata::Metadata;
use crate::standalone::standalone_module_loader::{EmbeddedModuleLoader, SharedModuleLoaderState};
use crate::RuntimeProviders;
use anyhow::Context;
use deno_config::workspace::{PackageJsonDepResolution, WorkspaceResolver};
use deno_core::error::AnyError;
use deno_core::url::Url;
use deno_core::{FastString, ModuleSpecifier};
use deno_npm::npm_rc::{RegistryConfigWithUrl, ResolvedNpmRc};
use deno_tls::rustls::RootCertStore;
use deno_tls::RootCertStoreProvider;
use eszip_async_trait::{
    AsyncEszipDataRead, NPM_RC_SCOPES_KEY, SOURCE_CODE_ESZIP_KEY, VFS_ESZIP_KEY,
};
use fs::deno_compile_fs::DenoCompileFileSystem;
use fs::{extract_static_files_from_eszip, load_npm_vfs};
use futures_util::future::OptionFuture;
use graph::resolver::{CjsResolutionStore, CliNodeResolver, NpmModuleLoader};
use graph::{eszip_migrate, payload_to_eszip, EszipPayloadKind, LazyLoadableEszip};
use import_map::{parse_from_json, ImportMap};
use npm::cache_dir::NpmCacheDir;
use npm::package_json::PackageJsonInstallDepsProvider;
use npm::{
    create_managed_npm_resolver, CliNpmResolverManagedCreateOptions,
    CliNpmResolverManagedSnapshotOption,
};
use sb_core::cache::caches::Caches;
use sb_core::cache::deno_dir::DenoDirProvider;
use sb_core::cache::node::NodeAnalysisCache;
use sb_core::cache::CacheSetting;
use sb_core::cert::{get_root_cert_store, CaData};
use sb_core::node::CliCjsCodeAnalyzer;
use sb_core::util::http_util::HttpClientProvider;
use sb_node::analyze::NodeCodeTranslator;
use sb_node::NodeResolver;
use standalone_module_loader::WorkspaceEszip;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
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

pub async fn create_module_loader_for_eszip<P>(
    mut eszip: LazyLoadableEszip,
    static_root_path: P,
    metadata: Metadata,
    maybe_import_map: Option<ImportMap>,
    include_source_map: bool,
) -> Result<RuntimeProviders, AnyError>
where
    P: AsRef<Path>,
{
    let current_exe_path = std::env::current_exe().unwrap();
    let current_exe_name = current_exe_path.file_name().unwrap().to_string_lossy();
    let deno_dir_provider = Arc::new(DenoDirProvider::new(None));
    let root_cert_store_provider = Arc::new(StandaloneRootCertStoreProvider {
        ca_stores: metadata.ca_stores,
        ca_data: metadata.ca_data.map(CaData::Bytes),
        cell: Default::default(),
    });

    let http_client_provider = Arc::new(HttpClientProvider::new(
        Some(root_cert_store_provider.clone()),
        metadata.unsafely_ignore_certificate_errors.clone(),
    ));

    // use a dummy npm registry url
    let npm_registry_url = ModuleSpecifier::parse("https://localhost/").unwrap();
    let root_path = if cfg!(target_family = "unix") {
        PathBuf::from("/var/tmp")
    } else {
        std::env::temp_dir()
    }
    .join(format!("sb-compile-{}", current_exe_name));

    let root_dir_url = ModuleSpecifier::from_directory_path(&root_path).unwrap();
    let root_node_modules_path = root_path.join("node_modules");

    let npm_cache_dir = NpmCacheDir::new(
        root_node_modules_path.clone(),
        vec![npm_registry_url.clone()],
    );

    let npm_global_cache_dir = npm_cache_dir.get_cache_location();
    let scopes = if let Some(maybe_scopes) = OptionFuture::<_>::from(
        eszip
            .ensure_module(NPM_RC_SCOPES_KEY)
            .map(|it| async move { it.take_source().await }),
    )
    .await
    .flatten()
    .map(|it| {
        rkyv::from_bytes::<HashMap<String, String>>(it.as_ref())
            .context("failed to deserialize npm scopes data from eszip")
    }) {
        maybe_scopes?
            .into_iter()
            .map(
                |(k, v)| -> Result<(String, RegistryConfigWithUrl), AnyError> {
                    Ok((
                        k,
                        RegistryConfigWithUrl {
                            registry_url: Url::parse(&v).context("failed to parse registry url")?,
                            config: Default::default(),
                        },
                    ))
                },
            )
            .collect::<Result<HashMap<_, _>, _>>()?
    } else {
        Default::default()
    };

    let entry_module_source = OptionFuture::<_>::from(
        eszip
            .ensure_module(SOURCE_CODE_ESZIP_KEY)
            .map(|it| async move { it.take_source().await }),
    )
    .await
    .flatten()
    .map(|it| String::from_utf8_lossy(it.as_ref()).into_owned())
    .map(FastString::from);

    let snapshot = eszip.take_npm_snapshot();
    let static_files = extract_static_files_from_eszip(&eszip, static_root_path).await;
    let vfs_root_dir_path = npm_cache_dir.root_dir().to_owned();

    let (fs, vfs) = {
        let vfs_data = OptionFuture::<_>::from(
            eszip
                .ensure_module(VFS_ESZIP_KEY)
                .map(|it| async move { it.source().await }),
        )
        .await
        .flatten();

        let vfs = load_npm_vfs(
            Arc::new(eszip.clone()),
            vfs_root_dir_path.clone(),
            vfs_data.as_deref(),
        )
        .context("Failed to load npm vfs.")?;

        let fs = DenoCompileFileSystem::new(vfs);
        let fs_backed_vfs = fs.file_backed_vfs().clone();

        (Arc::new(fs) as Arc<dyn deno_fs::FileSystem>, fs_backed_vfs)
    };

    let package_json_deps_provider = Arc::new(PackageJsonInstallDepsProvider::empty());
    let npm_resolver = create_managed_npm_resolver(CliNpmResolverManagedCreateOptions {
        snapshot: CliNpmResolverManagedSnapshotOption::Specified(snapshot.clone()),
        maybe_lockfile: None,
        fs: fs.clone(),
        http_client_provider,
        npm_global_cache_dir,
        cache_setting: CacheSetting::Use,
        maybe_node_modules_path: None,
        npm_system_info: Default::default(),
        package_json_deps_provider,
        npmrc: Arc::new(ResolvedNpmRc {
            default_config: deno_npm::npm_rc::RegistryConfigWithUrl {
                registry_url: npm_registry_url.clone(),
                config: Default::default(),
            },
            scopes,
            registry_configs: Default::default(),
        }),
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

    let cli_node_resolver = Arc::new(CliNodeResolver::new(
        cjs_resolutions.clone(),
        fs.clone(),
        node_resolver.clone(),
        npm_resolver.clone(),
    ));

    let module_loader_factory = StandaloneModuleLoaderFactory {
        shared: Arc::new(SharedModuleLoaderState {
            eszip: WorkspaceEszip {
                eszip,
                root_dir_url,
            },
            workspace_resolver: WorkspaceResolver::new_raw(
                maybe_import_map,
                vec![],
                PackageJsonDepResolution::Disabled,
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
        node_resolver,
        npm_resolver: npm_resolver.into_npm_resolver(),
        module_loader: Rc::new(EmbeddedModuleLoader {
            shared: module_loader_factory.shared.clone(),
            include_source_map,
        }),
        vfs,
        module_code: entry_module_source,
        static_files,
        npm_snapshot: snapshot,
        vfs_path: vfs_root_dir_path,
    })
}

pub async fn create_module_loader_for_standalone_from_eszip_kind<P>(
    eszip_payload_kind: EszipPayloadKind,
    static_root_path: P,
    maybe_import_map: Option<ImportMap>,
    maybe_import_map_path: Option<String>,
    include_source_map: bool,
) -> Result<RuntimeProviders, AnyError>
where
    P: AsRef<Path>,
{
    let eszip =
        eszip_migrate::try_migrate_if_needed(payload_to_eszip(eszip_payload_kind).await?).await?;
    let maybe_import_map = 'scope: {
        if maybe_import_map.is_some() {
            break 'scope maybe_import_map;
        } else if let Some(import_map_path) = maybe_import_map_path {
            let import_map_url = Url::parse(import_map_path.as_str())?;
            if let Some(import_map_module) = eszip.ensure_import_map(import_map_url.as_str()) {
                if let Some(source) = import_map_module.source().await {
                    let source = std::str::from_utf8(&source)?.to_string();
                    let result = parse_from_json(import_map_url, &source)?;

                    break 'scope Some(result.import_map);
                }
            }
        }

        None
    };

    create_module_loader_for_eszip(
        eszip,
        static_root_path,
        Metadata {
            ca_stores: None,
            ca_data: None,
            unsafely_ignore_certificate_errors: None,
        },
        maybe_import_map,
        include_source_map,
    )
    .await
}
