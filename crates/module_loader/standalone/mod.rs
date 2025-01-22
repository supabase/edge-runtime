use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::bail;
use anyhow::Context;
use deno::args::CacheSetting;
use deno::args::NpmInstallDepsProvider;
use deno::cache::Caches;
use deno::cache::DenoCacheEnvFsAdapter;
use deno::cache::DenoDirProvider;
use deno::cache::NodeAnalysisCache;
use deno::deno_cache_dir::npm::NpmCacheDir;
use deno::deno_npm;
use deno::deno_npm::npm_rc::RegistryConfigWithUrl;
use deno::deno_npm::npm_rc::ResolvedNpmRc;
use deno::deno_permissions::PermissionsOptions;
use deno::deno_resolver::cjs::IsCjsResolutionMode;
use deno::deno_resolver::npm::NpmReqResolverOptions;
use deno::deno_tls::rustls::RootCertStore;
use deno::deno_tls::RootCertStoreProvider;
use deno::http_util::HttpClientProvider;
use deno::node::CliCjsCodeAnalyzer;
use deno::node_resolver::analyze::NodeCodeTranslator;
use deno::node_resolver::PackageJsonResolver;
use deno::npm::create_in_npm_pkg_checker;
use deno::npm::create_managed_npm_resolver;
use deno::npm::CliManagedInNpmPkgCheckerCreateOptions;
use deno::npm::CliManagedNpmResolverCreateOptions;
use deno::npm::CliNpmResolverManagedSnapshotOption;
use deno::npm::CreateInNpmPkgCheckerOptions;
use deno::resolver::CjsTracker;
use deno::resolver::CliDenoResolverFs;
use deno::resolver::CliNpmReqResolver;
use deno::resolver::NpmModuleLoader;
use deno_config::workspace::PackageJsonDepResolution;
use deno_config::workspace::WorkspaceResolver;
use deno_core::error::AnyError;
use deno_core::url::Url;
use deno_core::FastString;
use deno_core::ModuleSpecifier;
use deno_facade::migrate;
use deno_facade::payload_to_eszip;
use deno_facade::EszipPayloadKind;
use deno_facade::LazyLoadableEszip;
use eszip_async_trait::AsyncEszipDataRead;
use eszip_async_trait::NPM_RC_SCOPES_KEY;
use eszip_async_trait::SOURCE_CODE_ESZIP_KEY;
use eszip_async_trait::VFS_ESZIP_KEY;
use ext_node::DenoFsNodeResolverEnv;
use ext_node::NodeResolver;
use ext_runtime::cert::get_root_cert_store;
use ext_runtime::cert::CaData;
use fs::deno_compile_fs::DenoCompileFileSystem;
use fs::extract_static_files_from_eszip;
use fs::load_npm_vfs;
use futures_util::future::OptionFuture;
use import_map::parse_from_json;
use import_map::ImportMap;
use standalone_module_loader::WorkspaceEszip;

use crate::metadata::Metadata;
use crate::standalone::standalone_module_loader::EmbeddedModuleLoader;
use crate::standalone::standalone_module_loader::SharedModuleLoaderState;
use crate::RuntimeProviders;

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
  base_dir_path: P,
  permissions: PermissionsOptions,
  metadata: Metadata,
  maybe_import_map: Option<ImportMap>,
  include_source_map: bool,
) -> Result<RuntimeProviders, AnyError>
where
  P: AsRef<Path>,
{
  let current_exe_path = std::env::current_exe().unwrap();
  let current_exe_name =
    current_exe_path.file_name().unwrap().to_string_lossy();
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

  let root_dir_url =
    Arc::new(ModuleSpecifier::from_directory_path(&root_path).unwrap());
  let root_node_modules_path = root_path.join("node_modules");
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
              registry_url: Url::parse(&v)
                .context("failed to parse registry url")?,
              config: Default::default(),
            },
          ))
        },
      )
      .collect::<Result<HashMap<_, _>, _>>()?
  } else {
    Default::default()
  };

  let npmrc = Arc::new(ResolvedNpmRc {
    default_config: deno_npm::npm_rc::RegistryConfigWithUrl {
      registry_url: npm_registry_url.clone(),
      config: Default::default(),
    },
    scopes,
    registry_configs: Default::default(),
  });

  let static_files =
    extract_static_files_from_eszip(&eszip, base_dir_path).await;

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
      root_node_modules_path.clone(),
      vfs_data.as_deref(),
    )
    .context("Failed to load npm vfs.")?;

    let fs = DenoCompileFileSystem::new(vfs);
    let fs_backed_vfs = fs.file_backed_vfs().clone();

    (
      Arc::new(fs) as Arc<dyn deno::deno_fs::FileSystem>,
      fs_backed_vfs,
    )
  };

  let npm_cache_dir = Arc::new(NpmCacheDir::new(
    &DenoCacheEnvFsAdapter(fs.as_ref()),
    root_node_modules_path,
    npmrc.get_all_known_registries_urls(),
  ));

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

  let pkg_json_resolver = Arc::new(PackageJsonResolver::new(
    ext_node::DenoFsNodeResolverEnv::new(fs.clone()),
  ));
  let in_npm_pkg_checker =
    create_in_npm_pkg_checker(CreateInNpmPkgCheckerOptions::Managed(
      CliManagedInNpmPkgCheckerCreateOptions {
        root_cache_dir_url: npm_cache_dir.root_dir_url(),
        maybe_node_modules_path: None,
      },
    ));

  let npm_resolver =
    create_managed_npm_resolver(CliManagedNpmResolverCreateOptions {
      snapshot: CliNpmResolverManagedSnapshotOption::Specified(
        snapshot.clone(),
      ),
      maybe_lockfile: None,
      fs: fs.clone(),
      http_client_provider,
      npm_cache_dir: npm_cache_dir.clone(),
      cache_setting: CacheSetting::Use,
      maybe_node_modules_path: None,
      npm_system_info: Default::default(),
      npm_install_deps_provider: Arc::new(
        // this is only used for installing packages, which isn't necessary with deno compile
        NpmInstallDepsProvider::empty(),
      ),
      npmrc,
    })
    .await?;

  let node_resolver = Arc::new(NodeResolver::new(
    DenoFsNodeResolverEnv::new(fs.clone()),
    in_npm_pkg_checker.clone(),
    npm_resolver.clone().into_npm_pkg_folder_resolver(),
    pkg_json_resolver.clone(),
  ));

  let cjs_tracker = Arc::new(CjsTracker::new(
    in_npm_pkg_checker.clone(),
    pkg_json_resolver.clone(),
    IsCjsResolutionMode::ExplicitTypeCommonJs,
  ));

  let cache_db = Caches::new(deno_dir_provider.clone());
  let node_analysis_cache = NodeAnalysisCache::new(cache_db.node_analysis_db());
  let npm_req_resolver =
    Arc::new(CliNpmReqResolver::new(NpmReqResolverOptions {
      byonm_resolver: (npm_resolver.clone()).into_maybe_byonm(),
      fs: CliDenoResolverFs(fs.clone()),
      in_npm_pkg_checker: in_npm_pkg_checker.clone(),
      node_resolver: node_resolver.clone(),
      npm_req_resolver: npm_resolver.clone().into_npm_req_resolver(),
    }));

  let cjs_esm_code_analyzer = CliCjsCodeAnalyzer::new(
    node_analysis_cache,
    cjs_tracker.clone(),
    fs.clone(),
    None,
  );
  let node_code_translator = Arc::new(NodeCodeTranslator::new(
    cjs_esm_code_analyzer,
    DenoFsNodeResolverEnv::new(fs.clone()),
    in_npm_pkg_checker,
    node_resolver.clone(),
    npm_resolver.clone().into_npm_pkg_folder_resolver(),
    pkg_json_resolver.clone(),
  ));

  let module_loader_factory = StandaloneModuleLoaderFactory {
    shared: Arc::new(SharedModuleLoaderState {
      eszip: WorkspaceEszip {
        eszip,
        root_dir_url: root_dir_url.clone(),
      },
      workspace_resolver: WorkspaceResolver::new_raw(
        root_dir_url,
        maybe_import_map,
        vec![],
        vec![],
        PackageJsonDepResolution::Disabled,
      ),
      cjs_tracker: cjs_tracker.clone(),
      npm_module_loader: Arc::new(NpmModuleLoader::new(
        cjs_tracker.clone(),
        fs.clone(),
        node_code_translator,
      )),
      npm_req_resolver,
      node_resolver: node_resolver.clone(),
    }),
  };

  Ok(RuntimeProviders {
    module_code: entry_module_source,
    module_loader: Rc::new(EmbeddedModuleLoader {
      shared: module_loader_factory.shared.clone(),
      include_source_map,
    }),
    node_services: RuntimeProviders::node_services_dummy(),
    npm_snapshot: snapshot,
    permissions: todo!(),
    static_files,
    vfs_path: npm_cache_dir.root_dir().to_path_buf(),
    vfs,
  })
}

pub async fn create_module_loader_for_standalone_from_eszip_kind<P>(
  eszip_payload_kind: EszipPayloadKind,
  base_dir_path: P,
  permissions: PermissionsOptions,
  maybe_import_map: Option<ImportMap>,
  maybe_import_map_path: Option<String>,
  include_source_map: bool,
) -> Result<RuntimeProviders, AnyError>
where
  P: AsRef<Path>,
{
  let eszip = match migrate::try_migrate_if_needed(
    payload_to_eszip(eszip_payload_kind).await?,
  )
  .await
  {
    Ok(v) => v,
    Err(_old) => {
      bail!("eszip migration failed");
    }
  };

  let maybe_import_map = 'scope: {
    if maybe_import_map.is_some() {
      break 'scope maybe_import_map;
    } else if let Some(import_map_path) = maybe_import_map_path {
      let import_map_url = Url::parse(import_map_path.as_str())?;
      if let Some(import_map_module) =
        eszip.ensure_import_map(import_map_url.as_str())
      {
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
    base_dir_path,
    permissions,
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
