use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use deno::args::CacheSetting;
use deno::args::CliLockfile;
use deno::cache::Caches;
use deno::cache::DenoCacheEnvFsAdapter;
use deno::cache::DenoDir;
use deno::cache::DenoDirProvider;
use deno::cache::EmitCache;
use deno::cache::FetchCacher;
use deno::cache::FetchCacherOptions;
use deno::cache::GlobalHttpCache;
use deno::cache::ModuleInfoCache;
use deno::cache::ParsedSourceCache;
use deno::deno_ast::EmitOptions;
use deno::deno_ast::SourceMapOption;
use deno::deno_ast::TranspileOptions;
use deno::deno_cache_dir::npm::NpmCacheDir;
use deno::deno_cache_dir::HttpCache;
use deno::deno_config::deno_json::JsxImportSourceConfig;
use deno::deno_config::workspace::WorkspaceResolver;
use deno::deno_core::futures::FutureExt;
use deno::deno_core::parking_lot::Mutex;
use deno::deno_lockfile::Lockfile;
use deno::deno_npm::npm_rc::ResolvedNpmRc;
use deno::deno_npm::resolution::ValidSerializedNpmResolutionSnapshot;
use deno::deno_resolver::cjs::IsCjsResolutionMode;
use deno::emit::Emitter;
use deno::file_fetcher::FileFetcher;
use deno::http_util::HttpClientProvider;
use deno::node_resolver::InNpmPackageChecker;
use deno::npm::create_in_npm_pkg_checker;
use deno::npm::create_managed_npm_resolver;
use deno::npm::CliManagedInNpmPkgCheckerCreateOptions;
use deno::npm::CliManagedNpmResolverCreateOptions;
use deno::npm::CliNpmResolver;
use deno::npm::CliNpmResolverManagedSnapshotOption;
use deno::npm::CreateInNpmPkgCheckerOptions;
use deno::npmrc::create_default_npmrc;
use deno::npmrc::create_npmrc;
use deno::resolver::CjsTracker;
use deno::DenoOptions;
use deno::PermissionsContainer;
use eszip::deno_graph::source::Loader;
use ext_node::DenoFsNodeResolverEnv;
use ext_node::NodeResolver;
use ext_node::PackageJsonResolver;
use import_map::ImportMap;

use crate::jsx_util::get_jsx_emit_opts;
use crate::jsx_util::get_rt_from_jsx;

use crate::DecoratorType;

struct Deferred<T>(once_cell::unsync::OnceCell<T>);

impl<T> Default for Deferred<T> {
  fn default() -> Self {
    Self(once_cell::unsync::OnceCell::default())
  }
}

impl<T> Deferred<T> {
  #[allow(dead_code)]
  pub fn get_or_try_init(
    &self,
    create: impl FnOnce() -> Result<T, anyhow::Error>,
  ) -> Result<&T, anyhow::Error> {
    self.0.get_or_try_init(create)
  }

  pub fn get_or_init(&self, create: impl FnOnce() -> T) -> &T {
    self.0.get_or_init(create)
  }

  #[allow(dead_code)]
  pub async fn get_or_try_init_async(
    &self,
    create: impl Future<Output = Result<T, anyhow::Error>>,
  ) -> Result<&T, anyhow::Error> {
    if self.0.get().is_none() {
      // todo(dsherret): it would be more ideal if this enforced a
      // single executor and then we could make some initialization
      // concurrent
      let val = create.await?;
      _ = self.0.set(val);
    }
    Ok(self.0.get().unwrap())
  }
}

#[derive(Clone)]
pub struct LockfileOpts {
  path: PathBuf,
  overwrite: bool,
}

pub struct EmitterFactory {
  // cjs_resolutions: Deferred<Arc<CjsResolutionStore>>,
  // cli_node_resolver: Deferred<Arc<CliNodeResolver>>,
  cache_strategy: Option<CacheSetting>,
  cjs_tracker: Deferred<Arc<CjsTracker>>,
  deno_dir: DenoDir,
  file_fetcher_allow_remote: bool,
  file_fetcher: Deferred<Arc<FileFetcher>>,
  global_http_cache: Deferred<Arc<GlobalHttpCache>>,
  deno_options: Deferred<Arc<dyn DenoOptions>>,
  http_client_provider: Deferred<Arc<HttpClientProvider>>,
  in_npm_pkg_checker: Deferred<Arc<dyn InNpmPackageChecker>>,
  jsx_import_source_config: Option<JsxImportSourceConfig>,
  lockfile: Deferred<Option<Arc<CliLockfile>>>,
  maybe_decorator: Option<DecoratorType>,
  maybe_lockfile: Option<LockfileOpts>,
  maybe_npmrc_env_vars: Option<HashMap<String, String>>,
  maybe_npmrc_path: Option<PathBuf>,
  module_info_cache: Deferred<Arc<ModuleInfoCache>>,
  node_resolver: Deferred<Arc<NodeResolver>>,
  npm_cache_dir: Deferred<Arc<NpmCacheDir>>,
  npm_resolver: Deferred<Arc<dyn CliNpmResolver>>,
  permissions: PermissionsContainer,
  pkg_json_resolver: Deferred<Arc<PackageJsonResolver>>,
  pub maybe_import_map: Option<ImportMap>,
  pub npm_snapshot: Option<ValidSerializedNpmResolutionSnapshot>,
  resolved_npm_rc: Deferred<Arc<ResolvedNpmRc>>,
  workspace_resolver: Deferred<Arc<WorkspaceResolver>>,
}

impl Default for EmitterFactory {
  fn default() -> Self {
    // Self::new()
    unreachable!()
  }
}

impl EmitterFactory {
  pub fn new(permissions: PermissionsContainer) -> Self {
    let deno_dir = DenoDir::new(None).unwrap();

    Self {
      // cjs_resolutions: Default::default(),
      // cli_node_resolver: Default::default(),
      cache_strategy: None,
      cjs_tracker: Default::default(),
      deno_dir,
      file_fetcher_allow_remote: true,
      file_fetcher: Default::default(),
      global_http_cache: Default::default(),
      http_client_provider: Default::default(),
      in_npm_pkg_checker: Default::default(),
      jsx_import_source_config: None,
      lockfile: Default::default(),
      maybe_decorator: None,
      maybe_import_map: None,
      maybe_lockfile: None,
      maybe_npmrc_env_vars: None,
      maybe_npmrc_path: None,
      module_info_cache: Default::default(),
      node_resolver: Default::default(),
      npm_cache_dir: Default::default(),
      npm_resolver: Default::default(),
      npm_snapshot: None,
      permissions,
      pkg_json_resolver: Default::default(),
      resolved_npm_rc: Default::default(),
      workspace_resolver: Default::default(),
      deno_options: Default::default(),
    }
  }

  pub fn set_cache_strategy(&mut self, strategy: CacheSetting) {
    self.cache_strategy = Some(strategy);
  }

  pub fn set_file_fetcher_allow_remote(&mut self, allow_remote: bool) {
    self.file_fetcher_allow_remote = allow_remote;
  }

  pub fn set_import_map(&mut self, import_map: Option<ImportMap>) {
    self.maybe_import_map = import_map;
  }

  pub fn set_jsx_import_source(&mut self, config: JsxImportSourceConfig) {
    self.jsx_import_source_config = Some(config);
  }

  pub fn set_npmrc_path<P>(&mut self, path: P)
  where
    P: AsRef<Path>,
  {
    self.maybe_npmrc_path = Some(path.as_ref().to_path_buf());
  }

  pub fn set_npmrc_env_vars(&mut self, vars: HashMap<String, String>) {
    self.maybe_npmrc_env_vars = Some(vars);
  }

  pub fn set_decorator_type(&mut self, decorator_type: Option<DecoratorType>) {
    self.maybe_decorator = decorator_type;
  }

  pub fn deno_dir_provider(&self) -> Arc<DenoDirProvider> {
    Arc::new(DenoDirProvider::new(None))
  }

  pub fn caches(&self) -> Result<Arc<Caches>, anyhow::Error> {
    let caches = Arc::new(Caches::new(self.deno_dir_provider()));
    let _ = caches.dep_analysis_db();
    let _ = caches.node_analysis_db();
    Ok(caches)
  }

  pub fn module_info_cache(
    &self,
  ) -> Result<&Arc<ModuleInfoCache>, anyhow::Error> {
    self.module_info_cache.get_or_try_init(|| {
      Ok(Arc::new(ModuleInfoCache::new(
        self.caches()?.dep_analysis_db(),
        self.parsed_source_cache()?,
      )))
    })
  }

  pub fn emit_cache(&self) -> Result<EmitCache, anyhow::Error> {
    Ok(EmitCache::new(self.deno_dir.gen_cache.clone()))
  }

  pub fn parsed_source_cache(
    &self,
  ) -> Result<Arc<ParsedSourceCache>, anyhow::Error> {
    let source_cache = Arc::new(ParsedSourceCache::default());
    Ok(source_cache)
  }

  pub fn emit_options(&self) -> EmitOptions {
    EmitOptions {
      inline_sources: true,
      source_map: SourceMapOption::Inline,
      ..Default::default()
    }
  }

  pub fn transpile_options(&self) -> TranspileOptions {
    let (specifier, module) =
      if let Some(jsx_config) = self.jsx_import_source_config.clone() {
        (jsx_config.default_specifier, jsx_config.module)
      } else {
        (None, "react".to_string())
      };

    let jsx_module = get_rt_from_jsx(Some(module));

    let (transform_jsx, jsx_automatic, jsx_development, precompile_jsx) =
      get_jsx_emit_opts(jsx_module.as_str());

    TranspileOptions {
      use_decorators_proposal: self
        .maybe_decorator
        .map(DecoratorType::is_use_decorators_proposal)
        .unwrap_or_default(),

      use_ts_decorators: self
        .maybe_decorator
        .map(DecoratorType::is_use_ts_decorators)
        .unwrap_or_default(),

      emit_metadata: self
        .maybe_decorator
        .map(DecoratorType::is_emit_metadata)
        .unwrap_or_default(),

      jsx_import_source: specifier,
      transform_jsx,
      jsx_automatic,
      jsx_development,
      precompile_jsx,
      ..Default::default()
    }
  }

  pub fn emitter(&self) -> Result<Arc<Emitter>, anyhow::Error> {
    let emitter = Arc::new(Emitter::new(
      self.cjs_tracker()?.clone(),
      Arc::new(self.emit_cache()?),
      self.parsed_source_cache()?,
      self.transpile_options(),
      self.emit_options(),
    ));

    Ok(emitter)
  }

  pub fn global_http_cache(&self) -> &Arc<GlobalHttpCache> {
    self.global_http_cache.get_or_init(|| {
      Arc::new(GlobalHttpCache::new(
        self.deno_dir.remote_folder_path(),
        deno::cache::RealDenoCacheEnv,
      ))
    })
  }

  pub fn http_cache(&self) -> Arc<dyn HttpCache> {
    self.global_http_cache().clone()
  }

  pub fn http_client_provider(&self) -> &Arc<HttpClientProvider> {
    self
      .http_client_provider
      .get_or_init(|| Arc::new(HttpClientProvider::new(None, None)))
  }

  pub fn real_fs(&self) -> Arc<dyn deno::deno_fs::FileSystem> {
    Arc::new(deno::deno_fs::RealFs)
  }

  pub fn get_lock_file_deferred(&self) -> &Option<Arc<CliLockfile>> {
    self.lockfile.get_or_init(|| {
      if let Some(lockfile_data) = self.maybe_lockfile.clone() {
        Some(Arc::new(Mutex::new(Lockfile::new_empty(
          lockfile_data.path.clone(),
          lockfile_data.overwrite,
        ))))
      } else {
        let default_lockfile_path = std::env::current_dir()
          .map(|p| p.join(".supabase.lock"))
          .unwrap();
        Some(Arc::new(Mutex::new(Lockfile::new_empty(
          default_lockfile_path,
          true,
        ))))
      }
    })
  }

  pub fn get_lock_file(&self) -> Option<Arc<CliLockfile>> {
    self.get_lock_file_deferred().as_ref().cloned()
  }

  pub fn cjs_tracker(&self) -> Result<&Arc<CjsTracker>, anyhow::Error> {
    self.cjs_tracker.get_or_try_init(|| {
      let options = self.deno_options();
      Ok(Arc::new(CjsTracker::new(
        self.in_npm_pkg_checker()?.clone(),
        self.pkg_json_resolver().clone(),
        if options.is_node_main() || options.unstable_detect_cjs() {
          IsCjsResolutionMode::ImplicitTypeCommonJs
        } else if options.detect_cjs() {
          IsCjsResolutionMode::ExplicitTypeCommonJs
        } else {
          IsCjsResolutionMode::Disabled
        },
      )))
    })
  }

  // pub fn cjs_resolutions(&self) -> &Arc<CjsResolutionStore> {
  //   self.cjs_resolutions.get_or_init(Default::default)
  // }

  pub fn in_npm_pkg_checker(
    &self,
  ) -> Result<&Arc<dyn InNpmPackageChecker>, anyhow::Error> {
    self.in_npm_pkg_checker.get_or_try_init(|| {
      let options = self.deno_options();
      let options = if options.use_byonm() {
        CreateInNpmPkgCheckerOptions::Byonm
      } else {
        CreateInNpmPkgCheckerOptions::Managed(
          CliManagedInNpmPkgCheckerCreateOptions {
            root_cache_dir_url: self.npm_cache_dir()?.root_dir_url(),
            maybe_node_modules_path: options
              .node_modules_dir_path()
              .map(|p| p.as_path()),
          },
        )
      };
      Ok(create_in_npm_pkg_checker(options))
    })
  }

  pub async fn npm_resolver(
    &self,
  ) -> Result<&Arc<dyn CliNpmResolver>, anyhow::Error> {
    self
      .npm_resolver
      .get_or_try_init_async(async {
        create_managed_npm_resolver(CliManagedNpmResolverCreateOptions {
          snapshot: CliNpmResolverManagedSnapshotOption::Specified(None),
          maybe_lockfile: self.get_lock_file(),
          fs: self.real_fs(),
          http_client_provider: self.http_client_provider().clone(),
          npm_cache_dir: self.npm_cache_dir()?.clone(),
          cache_setting: self
            .cache_strategy
            .clone()
            .unwrap_or(CacheSetting::Use),
          maybe_node_modules_path: None,
          npm_system_info: Default::default(),
          npm_install_deps_provider: Default::default(),
          npmrc: self.resolved_npm_rc().await?.clone(),
        })
        .await
      })
      .await
  }

  pub fn npm_cache_dir(&self) -> Result<&Arc<NpmCacheDir>, anyhow::Error> {
    self.npm_cache_dir.get_or_try_init(|| {
      let fs = self.real_fs();
      let global_path = self.deno_dir.npm_folder_path();
      let options = self.deno_options();
      Ok(Arc::new(NpmCacheDir::new(
        &DenoCacheEnvFsAdapter(fs.as_ref()),
        global_path,
        options.npmrc().get_all_known_registries_urls(),
      )))
    })
  }

  pub async fn resolved_npm_rc(
    &self,
  ) -> Result<&Arc<ResolvedNpmRc>, anyhow::Error> {
    self
      .resolved_npm_rc
      .get_or_try_init_async(async {
        if let Some(path) = self.maybe_npmrc_path.clone() {
          create_npmrc(path, self.maybe_npmrc_env_vars.as_ref()).await
        } else {
          Ok(create_default_npmrc())
        }
      })
      .await
  }

  pub async fn node_resolver(
    &self,
  ) -> Result<&Arc<NodeResolver>, anyhow::Error> {
    self
      .node_resolver
      .get_or_try_init_async(
        async {
          Ok(Arc::new(NodeResolver::new(
            DenoFsNodeResolverEnv::new(self.real_fs().clone()),
            self.in_npm_pkg_checker()?.clone(),
            self
              .npm_resolver()
              .await?
              .clone()
              .into_npm_pkg_folder_resolver(),
            self.pkg_json_resolver().clone(),
          )))
        }
        .boxed_local(),
      )
      .await
  }

  pub fn pkg_json_resolver(&self) -> &Arc<PackageJsonResolver> {
    self.pkg_json_resolver.get_or_init(|| {
      Arc::new(PackageJsonResolver::new(DenoFsNodeResolverEnv::new(
        self.real_fs().clone(),
      )))
    })
  }

  pub fn deno_options(&self) -> &Arc<dyn DenoOptions> {
    todo!()
  }

  // pub async fn cli_node_resolver(
  //   &self,
  // ) -> Result<&Arc<CliNodeResolver>, anyhow::Error> {
  //   self
  //     .cli_node_resolver
  //     .get_or_try_init_async(async {
  //       Ok(Arc::new(CliNodeResolver::new(
  //         self.cjs_resolutions().clone(),
  //         self.real_fs(),
  //         self.node_resolver().await?.clone(),
  //         self.npm_resolver().await?.clone(),
  //       )))
  //     })
  //     .await
  // }

  pub fn workspace_resolver(
    &self,
  ) -> Result<&Arc<WorkspaceResolver>, anyhow::Error> {
    self.workspace_resolver.get_or_try_init(|| {
      // Ok(Arc::new(WorkspaceResolver::new_raw(
      //   self.maybe_import_map.clone(),
      //   vec![],
      //   vec![],
      //   vec![],
      //   PackageJsonDepResolution::Disabled,
      // )))
      todo!()
    })
  }

  // pub async fn cli_graph_resolver(&self) -> &Arc<CliGraphResolver> {
  //   self
  //     .resolver
  //     .get_or_try_init_async(async {
  //       Ok(Arc::new(CliGraphResolver::new(CliGraphResolverOptions {
  //         node_resolver: Some(self.cli_node_resolver().await?.clone()),
  //         npm_resolver: if self.file_fetcher_allow_remote {
  //           Some(self.npm_resolver().await?.clone())
  //         } else {
  //           None
  //         },

  //         workspace_resolver: self.workspace_resolver()?.clone(),
  //         bare_node_builtins_enabled: false,
  //         maybe_jsx_import_source_config: self.jsx_import_source_config.clone(),
  //         maybe_vendor_dir: None,
  //       })))
  //     })
  //     .await
  //     .unwrap()
  // }

  pub fn file_fetcher(&self) -> Result<&Arc<FileFetcher>, anyhow::Error> {
    self.file_fetcher.get_or_try_init(|| {
      let http_client_provider = self.http_client_provider();
      let blob_store = Arc::new(deno::deno_web::BlobStore::default());

      Ok(Arc::new(FileFetcher::new(
        self.http_cache().clone(),
        self
          .cache_strategy
          .clone()
          .unwrap_or(CacheSetting::ReloadAll),
        self.file_fetcher_allow_remote,
        http_client_provider.clone(),
        blob_store,
      )))
    })
  }

  pub async fn file_fetcher_loader(
    &self,
  ) -> Result<Box<dyn Loader>, anyhow::Error> {
    Ok(Box::new(FetchCacher::new(
      self.file_fetcher()?.clone(),
      self.real_fs(),
      self.global_http_cache().clone(),
      self.in_npm_pkg_checker()?.clone(),
      self.module_info_cache()?.clone(),
      FetchCacherOptions {
        file_header_overrides: HashMap::new(),
        permissions: self.permissions.clone(),
        is_deno_publish: false,
      },
    )))
  }
}
