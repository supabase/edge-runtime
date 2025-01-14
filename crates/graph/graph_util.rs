use crate::emitter::EmitterFactory;
use crate::graph_fs::DenoGraphFsAdapter;
use crate::jsr::CliJsrUrlProvider;
// use crate::resolver::CliGraphResolver;
use anyhow::Context;
use deno::args::CliLockfile;
use deno::args::NpmCachingStrategy;
use deno::cache;
use deno::cache::ParsedSourceCache;
use deno::resolver::CjsTracker;
use deno::resolver::CliResolver;
use deno_config::deno_json::JsxImportSourceConfig;
use deno_core::error::custom_error;
use deno_core::error::AnyError;
use deno_core::parking_lot::Mutex;
use deno_core::FastString;
use deno_core::ModuleSpecifier;
use deno_fs::FileSystem;
use deno_graph::source::Loader;
use deno_graph::source::LoaderChecksum;
use deno_graph::source::ResolutionKind;
use deno_graph::source::ResolveError;
use deno_graph::GraphKind;
use deno_graph::JsrLoadError;
use deno_graph::ModuleError;
use deno_graph::ModuleGraph;
use deno_graph::ModuleGraphError;
use deno_graph::ModuleLoadError;
use deno_graph::ResolutionError;
use deno_graph::SpecifierError;
use deno_lockfile::Lockfile;
use deno_semver::package::PackageNv;
use deno_semver::package::PackageReq;
use eszip::EszipV2;
use import_map::ImportMapError;
use npm::CliNpmResolver;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Copy)]
pub struct GraphValidOptions {
  pub check_js: bool,
  pub follow_type_only: bool,
  pub is_vendoring: bool,
}

/// Check if `roots` and their deps are available. Returns `Ok(())` if
/// so. Returns `Err(_)` if there is a known module graph or resolution
/// error statically reachable from `roots`.
///
/// It is preferable to use this over using deno_graph's API directly
/// because it will have enhanced error message information specifically
/// for the CLI.
pub fn graph_valid(
  graph: &ModuleGraph,
  fs: &Arc<dyn FileSystem>,
  roots: &[ModuleSpecifier],
  options: GraphValidOptions,
) -> Result<(), AnyError> {
  let mut errors = graph
    .walk(
      roots.iter(),
      deno_graph::WalkOptions {
        check_js: options.check_js,
        follow_type_only: options.follow_type_only,
        follow_dynamic: options.is_vendoring,
        prefer_fast_check_graph: false,
      },
    )
    .errors()
    .flat_map(|error| {
      let is_root = match &error {
        ModuleGraphError::ResolutionError(_)
        | ModuleGraphError::TypesResolutionError(_) => false,
        ModuleGraphError::ModuleError(error) => {
          roots.contains(error.specifier())
        }
      };
      let mut message = match &error {
        ModuleGraphError::ResolutionError(resolution_error) => {
          enhanced_resolution_error_message(resolution_error)
        }
        ModuleGraphError::TypesResolutionError(resolution_error) => {
          format!(
            "Failed resolving types. {}",
            enhanced_resolution_error_message(resolution_error)
          )
        }
        ModuleGraphError::ModuleError(error) => {
          enhanced_lockfile_error_message(error)
            .or_else(|| enhanced_sloppy_imports_error_message(fs, error))
            .unwrap_or_else(|| format!("{}", error))
        }
      };

      if let Some(range) = error.maybe_range() {
        if !is_root && !range.specifier.as_str().contains("/$deno$eval") {
          message.push_str("\n    at ");
          message.push_str(&format_range(range));
        }
      }

      if graph.graph_kind() == GraphKind::TypesOnly
        && matches!(
          error,
          ModuleGraphError::ModuleError(ModuleError::UnsupportedMediaType(..))
        )
      {
        log::debug!("Ignoring: {}", message);
        return None;
      }

      if options.is_vendoring {
        // warn about failing dynamic imports when vendoring, but don't fail completely
        if matches!(
          error,
          ModuleGraphError::ModuleError(ModuleError::MissingDynamic(_, _))
        ) {
          log::warn!("Ignoring: {}", message);
          return None;
        }

        // ignore invalid downgrades and invalid local imports when vendoring
        match &error {
          ModuleGraphError::ResolutionError(err)
          | ModuleGraphError::TypesResolutionError(err) => {
            if matches!(
              err,
              ResolutionError::InvalidDowngrade { .. }
                | ResolutionError::InvalidLocalImport { .. }
            ) {
              return None;
            }
          }
          ModuleGraphError::ModuleError(_) => {}
        }
      }

      Some(custom_error(get_error_class_name(&error.into()), message))
    });
  if let Some(error) = errors.next() {
    Err(error)
  } else {
    // finally surface the npm resolution result
    if let Err(err) = &graph.npm_dep_graph_result {
      return Err(custom_error(get_error_class_name(err), format!("{}", err)));
    }
    Ok(())
  }
}

pub struct CreateGraphOptions<'a> {
  pub graph_kind: GraphKind,
  pub roots: Vec<ModuleSpecifier>,
  pub is_dynamic: bool,
  /// Specify `None` to use the default CLI loader.
  pub loader: Option<&'a mut dyn Loader>,
  pub npm_caching: NpmCachingStrategy,
}

pub struct ModuleGraphBuilder {
  type_check: bool,
  emitter_factory: Arc<EmitterFactory>,
}

impl ModuleGraphBuilder {
  pub fn new(emitter_factory: Arc<EmitterFactory>, type_check: bool) -> Self {
    Self {
      type_check,
      emitter_factory,
    }
  }

  pub async fn resolver(&self) -> Arc<CliGraphResolver> {
    self.emitter_factory.cli_graph_resolver().await.clone()
  }

  pub fn parsed_source_cache(&self) -> Arc<ParsedSourceCache> {
    self.emitter_factory.parsed_source_cache().unwrap().clone()
  }

  pub fn lockfile(&self) -> Option<Arc<Mutex<Lockfile>>> {
    self.emitter_factory.get_lock_file()
  }

  pub async fn npm_resolver(
    &self,
  ) -> Result<&Arc<dyn CliNpmResolver>, AnyError> {
    self.emitter_factory.npm_resolver().await
  }

  pub async fn build_graph_with_npm_resolution<'a>(
    &self,
    graph: &mut ModuleGraph,
    options: CreateGraphOptions<'a>,
  ) -> Result<(), AnyError> {
    enum MutLoaderRef<'a> {
      Borrowed(&'a mut dyn Loader),
      Owned(cache::FetchCacher),
    }

    impl<'a> MutLoaderRef<'a> {
      pub fn as_mut_loader(&mut self) -> &mut dyn Loader {
        match self {
          Self::Borrowed(loader) => *loader,
          Self::Owned(loader) => loader,
        }
      }
    }

    struct LockfileLocker<'a>(&'a CliLockfile);

    impl<'a> deno_graph::source::Locker for LockfileLocker<'a> {
      fn get_remote_checksum(
        &self,
        specifier: &deno_ast::ModuleSpecifier,
      ) -> Option<LoaderChecksum> {
        self
          .0
          .lock()
          .remote()
          .get(specifier.as_str())
          .map(|s| LoaderChecksum::new(s.clone()))
      }

      fn has_remote_checksum(
        &self,
        specifier: &deno_ast::ModuleSpecifier,
      ) -> bool {
        self.0.lock().remote().contains_key(specifier.as_str())
      }

      fn set_remote_checksum(
        &mut self,
        specifier: &deno_ast::ModuleSpecifier,
        checksum: LoaderChecksum,
      ) {
        self
          .0
          .lock()
          .insert_remote(specifier.to_string(), checksum.into_string())
      }

      fn get_pkg_manifest_checksum(
        &self,
        package_nv: &PackageNv,
      ) -> Option<LoaderChecksum> {
        self
          .0
          .lock()
          .content
          .packages
          .jsr
          .get(package_nv)
          .map(|s| LoaderChecksum::new(s.integrity.clone()))
      }

      fn set_pkg_manifest_checksum(
        &mut self,
        package_nv: &PackageNv,
        checksum: LoaderChecksum,
      ) {
        // a value would only exist in here if two workers raced
        // to insert the same package manifest checksum
        self
          .0
          .lock()
          .insert_package(package_nv.clone(), checksum.into_string());
      }
    }

    let maybe_imports = if options.graph_kind.include_types() {
      self.cli_options.to_compiler_option_types()?
    } else {
      Vec::new()
    };
    let analyzer = self.module_info_cache.as_module_analyzer();
    let mut loader = match options.loader {
      Some(loader) => MutLoaderRef::Borrowed(loader),
      None => MutLoaderRef::Owned(self.create_graph_loader()),
    };
    let cli_resolver = &self.resolver;
    let graph_resolver = self.create_graph_resolver()?;
    let graph_npm_resolver =
      cli_resolver.create_graph_npm_resolver(options.npm_caching);
    let maybe_file_watcher_reporter = self
      .maybe_file_watcher_reporter
      .as_ref()
      .map(|r| r.as_reporter());
    let mut locker = self
      .lockfile
      .as_ref()
      .map(|lockfile| LockfileLocker(lockfile));
    self
      .build_graph_with_npm_resolution_and_build_options(
        graph,
        options.roots,
        loader.as_mut_loader(),
        deno_graph::BuildOptions {
          imports: maybe_imports,
          is_dynamic: options.is_dynamic,
          passthrough_jsr_specifiers: false,
          executor: Default::default(),
          file_system: &DenoGraphFsAdapter(self.fs.as_ref()),
          jsr_url_provider: &CliJsrUrlProvider,
          npm_resolver: Some(&graph_npm_resolver),
          module_analyzer: &analyzer,
          reporter: maybe_file_watcher_reporter,
          resolver: Some(&graph_resolver),
          locker: locker.as_mut().map(|l| l as _),
        },
        options.npm_caching,
      )
      .await
  }

  #[allow(clippy::borrow_deref_ref)]
  pub async fn create_graph_and_maybe_check(
    &self,
    roots: Vec<ModuleSpecifier>,
  ) -> Result<deno_graph::ModuleGraph, AnyError> {
    let mut cache = self.emitter_factory.file_fetcher_loader().await?;
    let cli_resolver = self.resolver().await.clone();
    let graph_resolver = cli_resolver.as_graph_resolver();
    let graph_npm_resolver = cli_resolver.create_graph_npm_resolver();
    let psc = self.parsed_source_cache();
    let analyzer = self
      .emitter_factory
      .module_info_cache()
      .unwrap()
      .as_module_analyzer(&psc);

    let graph_kind = deno_graph::GraphKind::CodeOnly;
    let mut graph = ModuleGraph::new(graph_kind);
    let fs = Arc::new(deno_fs::RealFs);
    let fs = DenoGraphFsAdapter(fs.as_ref());
    let lockfile = self.lockfile();
    let mut locker = lockfile.as_ref().map(LockfileLocker);

    self
      .build_graph_with_npm_resolution(
        &mut graph,
        roots,
        cache.as_mut(),
        deno_graph::BuildOptions {
          is_dynamic: false,
          imports: vec![],
          executor: Default::default(),
          file_system: &fs,
          jsr_url_provider: &CliJsrUrlProvider,
          resolver: Some(&*graph_resolver),
          npm_resolver: Some(&graph_npm_resolver),
          module_analyzer: &analyzer,
          reporter: None,
          workspace_members: &[],
          passthrough_jsr_specifiers: false,
          locker: locker.as_mut().map(|l| l as _),
        },
      )
      .await?;

    self.graph_valid(&graph)?;

    Ok(graph)
  }

  /// Check if `roots` and their deps are available. Returns `Ok(())` if
  /// so. Returns `Err(_)` if there is a known module graph or resolution
  /// error statically reachable from `roots` and not a dynamic import.
  pub fn graph_valid(&self, graph: &ModuleGraph) -> Result<(), AnyError> {
    self.graph_roots_valid(
      graph,
      &graph.roots.iter().cloned().collect::<Vec<_>>(),
    )
  }

  pub fn graph_roots_valid(
    &self,
    graph: &ModuleGraph,
    roots: &[ModuleSpecifier],
  ) -> Result<(), AnyError> {
    let fs: Arc<dyn FileSystem> = Arc::new(deno_fs::RealFs);

    graph_valid(
      graph,
      &fs,
      roots,
      GraphValidOptions {
        is_vendoring: false,
        follow_type_only: true,
        check_js: false,
      },
    )
  }
}

#[allow(clippy::arc_with_non_send_sync)]
pub async fn create_eszip_from_graph_raw(
  graph: ModuleGraph,
  emitter_factory: Option<Arc<EmitterFactory>>,
) -> Result<EszipV2, AnyError> {
  let emitter =
    emitter_factory.unwrap_or_else(|| Arc::new(EmitterFactory::new()));
  let parser_arc = emitter.clone().parsed_source_cache().unwrap();
  let parser = parser_arc.as_capturing_parser();
  let transpile_options = emitter.transpile_options();
  let emit_options = emitter.emit_options();

  eszip::EszipV2::from_graph(eszip::FromGraphOptions {
    graph,
    parser,
    transpile_options,
    emit_options,
    relative_file_base: None,
    npm_packages: None,
  })
}

pub async fn create_graph(
  file: PathBuf,
  emitter_factory: Arc<EmitterFactory>,
  maybe_code: &Option<FastString>,
) -> Result<ModuleGraph, AnyError> {
  let module_specifier = if let Some(code) = maybe_code {
    let specifier = ModuleSpecifier::parse("file:///src/index.ts").unwrap();

    emitter_factory.file_fetcher()?.insert_memory_files(File {
      specifier: specifier.clone(),
      maybe_headers: None,
      source: code.as_bytes().into(),
    });

    specifier
  } else {
    let binding =
      std::fs::canonicalize(&file).context("failed to read path")?;
    let specifier =
      binding.to_str().context("failed to convert path to str")?;
    let format_specifier = format!("file:///{}", specifier);

    ModuleSpecifier::parse(&format_specifier)
      .context("failed to parse specifier")?
  };

  let builder = ModuleGraphBuilder::new(emitter_factory, false);
  let create_module_graph_task =
    builder.create_graph_and_maybe_check(vec![module_specifier]);

  create_module_graph_task
    .await
    .context("failed to create the graph")
}

/// Adds more explanatory information to a resolution error.
pub fn enhanced_resolution_error_message(error: &ResolutionError) -> String {
  let message = format!("{error}");

  // if let Some(specifier) = get_resolution_error_bare_node_specifier(error) {
  //     if !*DENO_DISABLE_PEDANTIC_NODE_WARNINGS {
  //         message.push_str(&format!(
  //             "\nIf you want to use a built-in Node module, add a \"node:\" prefix (ex. \"node:{specifier}\")."
  //         ));
  //     }
  // }

  message
}

fn enhanced_sloppy_imports_error_message(
  _fs: &Arc<dyn FileSystem>,
  error: &ModuleError,
) -> Option<String> {
  match error {
        ModuleError::LoadingErr(_specifier, _, ModuleLoadError::Loader(_)) // ex. "Is a directory" error
        | ModuleError::Missing(_specifier, _) => Some(format!("{}", error)),
        _ => None,
    }
}

fn enhanced_lockfile_error_message(err: &ModuleError) -> Option<String> {
  match err {
      ModuleError::LoadingErr(
        specifier,
        _,
        ModuleLoadError::Jsr(JsrLoadError::ContentChecksumIntegrity(
          checksum_err,
        )),
      ) => {
        Some(format!(
          concat!(
            "Integrity check failed in package. The package may have been tampered with.\n\n",
            "  Specifier: {}\n",
            "  Actual: {}\n",
            "  Expected: {}\n\n",
            "If you modified your global cache, run again with the --reload flag to restore ",
            "its state. If you want to modify dependencies locally run again with the ",
            "--vendor flag or specify `\"vendor\": true` in a deno.json then modify the contents ",
            "of the vendor/ folder."
          ),
          specifier,
          checksum_err.actual,
          checksum_err.expected,
        ))
      }
      ModuleError::LoadingErr(
        _specifier,
        _,
        ModuleLoadError::Jsr(
          JsrLoadError::PackageVersionManifestChecksumIntegrity(
            package_nv,
            checksum_err,
          ),
        ),
      ) => {
        Some(format!(
          concat!(
            "Integrity check failed for package. The source code is invalid, as it does not match the expected hash in the lock file.\n\n",
            "  Package: {}\n",
            "  Actual: {}\n",
            "  Expected: {}\n\n",
            "This could be caused by:\n",
            "  * the lock file may be corrupt\n",
            "  * the source itself may be corrupt\n\n",
            "Use the --lock-write flag to regenerate the lockfile or --reload to reload the source code from the server."
          ),
          package_nv,
          checksum_err.actual,
          checksum_err.expected,
        ))
      }
      ModuleError::LoadingErr(
        specifier,
        _,
        ModuleLoadError::HttpsChecksumIntegrity(checksum_err),
      ) => {
        Some(format!(
          concat!(
            "Integrity check failed for remote specifier. The source code is invalid, as it does not match the expected hash in the lock file.\n\n",
            "  Specifier: {}\n",
            "  Actual: {}\n",
            "  Expected: {}\n\n",
            "This could be caused by:\n",
            "  * the lock file may be corrupt\n",
            "  * the source itself may be corrupt\n\n",
            "Use the --lock-write flag to regenerate the lockfile or --reload to reload the source code from the server."
          ),
          specifier,
          checksum_err.actual,
          checksum_err.expected,
        ))
      }
      _ => None,
    }
}

pub fn get_resolution_error_bare_node_specifier(
  error: &ResolutionError,
) -> Option<&str> {
  get_resolution_error_bare_specifier(error)
    .filter(|specifier| ext_node::is_builtin_node_module(specifier))
}

fn get_resolution_error_bare_specifier(
  error: &ResolutionError,
) -> Option<&str> {
  if let ResolutionError::InvalidSpecifier {
    error: SpecifierError::ImportPrefixMissing { specifier, .. },
    ..
  } = error
  {
    Some(specifier.as_str())
  } else if let ResolutionError::ResolverError { error, .. } = error {
    if let ResolveError::Other(error) = (*error).as_ref() {
      if let Some(ImportMapError::UnmappedBareSpecifier(specifier, _)) =
        error.downcast_ref::<ImportMapError>()
      {
        Some(specifier.as_str())
      } else {
        None
      }
    } else {
      None
    }
  } else {
    None
  }
}

pub fn format_range(range: &deno_graph::Range) -> String {
  format!(
    "{}:{}:{}",
    range.specifier.as_str(),
    &(range.start.line + 1).to_string(),
    &(range.start.character + 1).to_string()
  )
}

struct LockfileLocker<'a>(&'a Arc<CliLockfile>);

impl<'a> deno_graph::source::Locker for LockfileLocker<'a> {
  fn get_remote_checksum(
    &self,
    specifier: &deno_ast::ModuleSpecifier,
  ) -> Option<LoaderChecksum> {
    self
      .0
      .lock()
      .remote()
      .get(specifier.as_str())
      .map(|s| LoaderChecksum::new(s.clone()))
  }

  fn has_remote_checksum(&self, specifier: &deno_ast::ModuleSpecifier) -> bool {
    self.0.lock().remote().contains_key(specifier.as_str())
  }

  fn set_remote_checksum(
    &mut self,
    specifier: &deno_ast::ModuleSpecifier,
    checksum: LoaderChecksum,
  ) {
    self
      .0
      .lock()
      .insert_remote(specifier.to_string(), checksum.into_string())
  }

  fn get_pkg_manifest_checksum(
    &self,
    package_nv: &PackageNv,
  ) -> Option<LoaderChecksum> {
    self
      .0
      .lock()
      .content
      .packages
      .jsr
      .get(&package_nv.to_string())
      .map(|s| LoaderChecksum::new(s.integrity.clone()))
  }

  fn set_pkg_manifest_checksum(
    &mut self,
    package_nv: &PackageNv,
    checksum: LoaderChecksum,
  ) {
    // a value would only exist in here if two workers raced
    // to insert the same package manifest checksum
    self
      .0
      .lock()
      .insert_package(package_nv.to_string(), checksum.into_string());
  }
}

#[derive(Debug)]
struct CliGraphResolver<'a> {
  cjs_tracker: &'a CjsTracker,
  resolver: &'a CliResolver,
  jsx_import_source_config: Option<JsxImportSourceConfig>,
}

impl<'a> deno_graph::source::Resolver for CliGraphResolver<'a> {
  fn default_jsx_import_source(&self) -> Option<String> {
    self
      .jsx_import_source_config
      .as_ref()
      .and_then(|c| c.default_specifier.clone())
  }

  fn default_jsx_import_source_types(&self) -> Option<String> {
    self
      .jsx_import_source_config
      .as_ref()
      .and_then(|c| c.default_types_specifier.clone())
  }

  fn jsx_import_source_module(&self) -> &str {
    self
      .jsx_import_source_config
      .as_ref()
      .map(|c| c.module.as_str())
      .unwrap_or(deno_graph::source::DEFAULT_JSX_IMPORT_SOURCE_MODULE)
  }

  fn resolve(
    &self,
    raw_specifier: &str,
    referrer_range: &deno_graph::Range,
    resolution_kind: ResolutionKind,
  ) -> Result<ModuleSpecifier, ResolveError> {
    self.resolver.resolve(
      raw_specifier,
      &referrer_range.specifier,
      referrer_range.range.start,
      referrer_range
        .resolution_mode
        .map(to_node_resolution_mode)
        .unwrap_or_else(|| {
          self
            .cjs_tracker
            .get_referrer_kind(&referrer_range.specifier)
        }),
      to_node_resolution_kind(resolution_kind),
    )
  }
}

pub fn to_node_resolution_kind(
  kind: ResolutionKind,
) -> node_resolver::NodeResolutionKind {
  match kind {
    ResolutionKind::Execution => node_resolver::NodeResolutionKind::Execution,
    ResolutionKind::Types => node_resolver::NodeResolutionKind::Types,
  }
}

pub fn to_node_resolution_mode(
  mode: deno_graph::source::ResolutionMode,
) -> node_resolver::ResolutionMode {
  match mode {
    deno_graph::source::ResolutionMode::Import => {
      node_resolver::ResolutionMode::Import
    }
    deno_graph::source::ResolutionMode::Require => {
      node_resolver::ResolutionMode::Require
    }
  }
}
