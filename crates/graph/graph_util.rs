use crate::emitter::EmitterFactory;
use crate::graph_fs::DenoGraphFsAdapter;
use crate::jsr::CliJsrUrlProvider;
use crate::resolver::CliGraphResolver;
use anyhow::{anyhow, Context};
use deno_core::error::{custom_error, AnyError};
use deno_core::parking_lot::Mutex;
use deno_core::{FastString, ModuleSpecifier};
use deno_fs::FileSystem;
use deno_graph::source::{Loader, LoaderChecksum, ResolveError};
use deno_graph::{
    GraphKind, JsrLoadError, ModuleError, ModuleGraph, ModuleLoadError, SpecifierError,
};
use deno_graph::{ModuleGraphError, ResolutionError};
use deno_lockfile::Lockfile;
use deno_semver::package::{PackageNv, PackageReq};
use eszip::EszipV2;
use import_map::ImportMapError;
use npm::CliNpmResolver;
use npm_cache::file_fetcher::File;
use sb_core::cache::parsed_source::ParsedSourceCache;
use sb_core::util::errors::get_error_class_name;
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
                ModuleGraphError::ModuleError(error) => roots.contains(error.specifier()),
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
                ModuleGraphError::ModuleError(error) => enhanced_lockfile_error_message(error)
                    .or_else(|| enhanced_sloppy_imports_error_message(fs, error))
                    .unwrap_or_else(|| format!("{}", error)),
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

    pub async fn npm_resolver(&self) -> Result<&Arc<dyn CliNpmResolver>, AnyError> {
        self.emitter_factory.npm_resolver().await
    }

    pub async fn create_graph_with_loader(
        &self,
        graph_kind: GraphKind,
        roots: Vec<ModuleSpecifier>,
        loader: &mut dyn Loader,
    ) -> Result<deno_graph::ModuleGraph, AnyError> {
        let cli_resolver = self.resolver().await;
        let graph_resolver = cli_resolver.as_graph_resolver();
        let graph_npm_resolver = cli_resolver.create_graph_npm_resolver();
        let psc = self.parsed_source_cache();
        let analyzer = self
            .emitter_factory
            .module_info_cache()
            .unwrap()
            .as_module_analyzer(&psc);

        let mut graph = ModuleGraph::new(graph_kind);
        let fs = Arc::new(deno_fs::RealFs);
        let fs = DenoGraphFsAdapter(fs.as_ref());
        let lockfile = self.lockfile();
        let mut locker = lockfile.as_ref().map(LockfileLocker);

        self.build_graph_with_npm_resolution(
            &mut graph,
            roots,
            loader,
            deno_graph::BuildOptions {
                is_dynamic: false,
                imports: vec![],
                executor: Default::default(),
                file_system: &fs,
                jsr_url_provider: &CliJsrUrlProvider,
                resolver: Some(graph_resolver),
                npm_resolver: Some(&graph_npm_resolver),
                module_analyzer: &analyzer,
                reporter: None,
                workspace_members: &[],
                passthrough_jsr_specifiers: false,
                locker: locker.as_mut().map(|l| l as _),
            },
        )
        .await?;

        if graph.has_node_specifier && self.type_check {
            self.npm_resolver()
                .await?
                .as_managed()
                .unwrap()
                .inject_synthetic_types_node_package()
                .await?;
        }

        Ok(graph)
    }

    pub async fn build_graph_with_npm_resolution<'a>(
        &self,
        graph: &mut ModuleGraph,
        roots: Vec<ModuleSpecifier>,
        loader: &'a mut dyn deno_graph::source::Loader,
        options: deno_graph::BuildOptions<'a>,
    ) -> Result<(), AnyError> {
        // fill the graph with the information from the lockfile
        let is_first_execution = graph.roots.is_empty();
        if is_first_execution {
            // populate the information from the lockfile
            if let Some(lockfile) = &self.lockfile() {
                let lockfile = lockfile.lock();
                for (from, to) in &lockfile.content.redirects {
                    if let Ok(from) = ModuleSpecifier::parse(from) {
                        if let Ok(to) = ModuleSpecifier::parse(to) {
                            if !matches!(from.scheme(), "file" | "npm" | "jsr") {
                                graph.redirects.insert(from, to);
                            }
                        }
                    }
                }
                for (key, value) in &lockfile.content.packages.specifiers {
                    if let Some(key) = key
                        .strip_prefix("jsr:")
                        .and_then(|key| PackageReq::from_str(key).ok())
                    {
                        if let Some(value) = value
                            .strip_prefix("jsr:")
                            .and_then(|value| PackageNv::from_str(value).ok())
                        {
                            graph.packages.add_nv(key, value);
                        }
                    }
                }
            }
        }

        let initial_redirects_len = graph.redirects.len();
        let initial_package_deps_len = graph.packages.package_deps_sum();
        let initial_package_mappings_len = graph.packages.mappings().len();

        graph.build(roots, loader, options).await;

        let has_redirects_changed = graph.redirects.len() != initial_redirects_len;
        let has_jsr_package_deps_changed =
            graph.packages.package_deps_sum() != initial_package_deps_len;
        let has_jsr_package_mappings_changed =
            graph.packages.mappings().len() != initial_package_mappings_len;

        if has_redirects_changed || has_jsr_package_deps_changed || has_jsr_package_mappings_changed
        {
            if let Some(lockfile) = &self.lockfile() {
                let mut lockfile = lockfile.lock();
                // https redirects
                if has_redirects_changed {
                    let graph_redirects = graph
                        .redirects
                        .iter()
                        .filter(|(from, _)| !matches!(from.scheme(), "npm" | "file" | "deno"));
                    for (from, to) in graph_redirects {
                        lockfile.insert_redirect(from.to_string(), to.to_string());
                    }
                }
                // jsr package mappings
                if has_jsr_package_mappings_changed {
                    for (from, to) in graph.packages.mappings() {
                        lockfile.insert_package_specifier(
                            format!("jsr:{}", from),
                            format!("jsr:{}", to),
                        );
                    }
                }
                // jsr packages
                if has_jsr_package_deps_changed {
                    for (name, deps) in graph.packages.packages_with_deps() {
                        lockfile.add_package_deps(&name.to_string(), deps.map(|s| s.to_string()));
                    }
                }
            }
        }

        Ok(())
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

        self.build_graph_with_npm_resolution(
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
        self.graph_roots_valid(graph, &graph.roots.iter().cloned().collect::<Vec<_>>())
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
    let emitter = emitter_factory.unwrap_or_else(|| Arc::new(EmitterFactory::new()));
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
        let binding = std::fs::canonicalize(&file).context("failed to read path")?;

        ModuleSpecifier::from_file_path(binding)
            .map_err(|_| anyhow!("failed to parse specifier"))?
    };

    let builder = ModuleGraphBuilder::new(emitter_factory, false);
    let create_module_graph_task = builder.create_graph_and_maybe_check(vec![module_specifier]);

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

pub fn get_resolution_error_bare_node_specifier(error: &ResolutionError) -> Option<&str> {
    get_resolution_error_bare_specifier(error)
        .filter(|specifier| sb_node::is_builtin_node_module(specifier))
}

fn get_resolution_error_bare_specifier(error: &ResolutionError) -> Option<&str> {
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

struct LockfileLocker<'a>(&'a Arc<Mutex<Lockfile>>);

impl<'a> deno_graph::source::Locker for LockfileLocker<'a> {
    fn get_remote_checksum(&self, specifier: &deno_ast::ModuleSpecifier) -> Option<LoaderChecksum> {
        self.0
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
        self.0
            .lock()
            .insert_remote(specifier.to_string(), checksum.into_string())
    }

    fn get_pkg_manifest_checksum(&self, package_nv: &PackageNv) -> Option<LoaderChecksum> {
        self.0
            .lock()
            .content
            .packages
            .jsr
            .get(&package_nv.to_string())
            .map(|s| LoaderChecksum::new(s.integrity.clone()))
    }

    fn set_pkg_manifest_checksum(&mut self, package_nv: &PackageNv, checksum: LoaderChecksum) {
        // a value would only exist in here if two workers raced
        // to insert the same package manifest checksum
        self.0
            .lock()
            .insert_package(package_nv.to_string(), checksum.into_string());
    }
}
