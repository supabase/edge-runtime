use crate::emitter::EmitterFactory;
use crate::graph_resolver::CliGraphResolver;
use deno_ast::MediaType;
use deno_core::error::{custom_error, AnyError};
use deno_core::parking_lot::Mutex;
use deno_core::{FastString, ModuleSpecifier};
use deno_graph::ModuleError;
use deno_graph::ResolutionError;
use deno_lockfile::Lockfile;
use deno_semver::package::{PackageNv, PackageReq};
use eszip::deno_graph::source::Loader;
use eszip::deno_graph::{GraphKind, ModuleGraph, ModuleGraphError};
use eszip::{deno_graph, EszipV2};
use sb_core::cache::parsed_source::ParsedSourceCache;
use sb_core::errors_rt::get_error_class_name;
use sb_core::file_fetcher::File;
use sb_npm::CliNpmResolver;
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
/// error statically reachable from `roots` and not a dynamic import.
pub fn graph_valid_with_cli_options(
    graph: &ModuleGraph,
    roots: &[ModuleSpecifier],
) -> Result<(), AnyError> {
    graph_valid(
        graph,
        roots,
        GraphValidOptions {
            is_vendoring: false,
            follow_type_only: true,
            check_js: false,
        },
    )
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

    pub async fn npm_resolver(&self) -> &Arc<dyn CliNpmResolver> {
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
        let graph_npm_resolver = cli_resolver.as_graph_npm_resolver();
        let analyzer = self.parsed_source_cache().as_analyzer();

        let mut graph = ModuleGraph::new(graph_kind);
        self.build_graph_with_npm_resolution(
            &mut graph,
            roots,
            loader,
            deno_graph::BuildOptions {
                is_dynamic: false,
                imports: vec![],
                file_system: None,
                resolver: Some(graph_resolver),
                npm_resolver: Some(graph_npm_resolver),
                module_analyzer: Some(&*analyzer),
                reporter: None,
                // todo(dsherret): workspace support
                workspace_members: vec![],
            },
        )
        .await?;

        if graph.has_node_specifier && self.type_check {
            self.npm_resolver()
                .await
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
        loader: &mut dyn deno_graph::source::Loader,
        options: deno_graph::BuildOptions<'a>,
    ) -> Result<(), AnyError> {
        // TODO: Option here similar to: https://github.com/denoland/deno/blob/v1.37.1/cli/graph_util.rs#L323C5-L405C11
        // self.resolver.force_top_level_package_json_install().await?; TODO
        // add the lockfile redirects to the graph if it's the first time executing
        if graph.redirects.is_empty() {
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
            }
        }

        // add the jsr specifiers to the graph if it's the first time executing
        if graph.packages.is_empty() {
            if let Some(lockfile) = &self.lockfile() {
                let lockfile = lockfile.lock();
                for (key, value) in &lockfile.content.packages.specifiers {
                    if let Some(key) = key
                        .strip_prefix("jsr:")
                        .and_then(|key| PackageReq::from_str(key).ok())
                    {
                        if let Some(value) = value
                            .strip_prefix("jsr:")
                            .and_then(|value| PackageNv::from_str(value).ok())
                        {
                            graph.packages.add(key, value);
                        }
                    }
                }
            }
        }

        graph.build(roots, loader, options).await;

        // add the redirects in the graph to the lockfile
        if !graph.redirects.is_empty() {
            if let Some(lockfile) = &self.lockfile() {
                let graph_redirects = graph
                    .redirects
                    .iter()
                    .filter(|(from, _)| !matches!(from.scheme(), "npm" | "file" | "deno"));
                let mut lockfile = lockfile.lock();
                for (from, to) in graph_redirects {
                    lockfile.insert_redirect(from.to_string(), to.to_string());
                }
            }
        }

        // add the jsr specifiers in the graph to the lockfile
        if !graph.packages.is_empty() {
            if let Some(lockfile) = &self.lockfile() {
                let mappings = graph.packages.mappings();
                let mut lockfile = lockfile.lock();
                for (from, to) in mappings {
                    lockfile
                        .insert_package_specifier(format!("jsr:{}", from), format!("jsr:{}", to));
                }
            }
        }

        if let Some(npm_resolver) = self.npm_resolver().await.as_managed() {
            // ensure that the top level package.json is installed if a
            // specifier was matched in the package.json
            if self.resolver().await.found_package_json_dep() {
                npm_resolver.ensure_top_level_package_json_install().await?;
            }

            // resolve the dependencies of any pending dependencies
            // that were inserted by building the graph
            npm_resolver.resolve_pending().await?;
        }

        Ok(())
    }

    #[allow(clippy::borrow_deref_ref)]
    pub async fn create_graph_and_maybe_check(
        &self,
        roots: Vec<ModuleSpecifier>,
    ) -> Result<deno_graph::ModuleGraph, AnyError> {
        //
        let mut cache = self.emitter_factory.file_fetcher_loader();
        let cli_resolver = self.resolver().await.clone();
        let graph_resolver = cli_resolver.as_graph_resolver();
        let graph_npm_resolver = cli_resolver.as_graph_npm_resolver();
        let analyzer = self.parsed_source_cache().as_analyzer();
        let graph_kind = deno_graph::GraphKind::CodeOnly;
        let mut graph = ModuleGraph::new(graph_kind);

        self.build_graph_with_npm_resolution(
            &mut graph,
            roots,
            cache.as_mut(),
            deno_graph::BuildOptions {
                is_dynamic: false,
                imports: vec![],
                file_system: None,
                resolver: Some(&*graph_resolver),
                npm_resolver: Some(&*graph_npm_resolver),
                module_analyzer: Some(&*analyzer),
                reporter: None,
                workspace_members: vec![],
            },
        )
        .await?;

        Ok(graph)
    }
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
    roots: &[ModuleSpecifier],
    options: GraphValidOptions,
) -> Result<(), AnyError> {
    let mut errors = graph
        .walk(
            roots,
            deno_graph::WalkOptions {
                check_js: options.check_js,
                follow_type_only: options.follow_type_only,
                follow_dynamic: options.is_vendoring,
            },
        )
        .errors()
        .flat_map(|error| {
            let _is_root = match &error {
                ModuleGraphError::ResolutionError(_) => false,
                ModuleGraphError::ModuleError(error) => roots.contains(error.specifier()),
                _ => false,
            };
            let message = if let ModuleGraphError::ResolutionError(_err) = &error {
                format!("{error}")
            } else {
                format!("{error}")
            };

            if options.is_vendoring {
                // warn about failing dynamic imports when vendoring, but don't fail completely
                if matches!(
                    error,
                    ModuleGraphError::ModuleError(ModuleError::MissingDynamic(_, _))
                ) {
                    log::warn!("Ignoring: {:#}", message);
                    return None;
                }

                // ignore invalid downgrades and invalid local imports when vendoring
                if let ModuleGraphError::ResolutionError(err) = &error {
                    if matches!(
                        err,
                        ResolutionError::InvalidDowngrade { .. }
                            | ResolutionError::InvalidLocalImport { .. }
                    ) {
                        return None;
                    }
                }
            }

            Some(custom_error(
                get_error_class_name(&error.into()).unwrap(),
                message,
            ))
        });
    if let Some(error) = errors.next() {
        Err(error)
    } else {
        Ok(())
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

    eszip::EszipV2::from_graph(graph, &parser, Default::default())
}

pub async fn create_graph(
    file: PathBuf,
    emitter_factory: Arc<EmitterFactory>,
    maybe_code: &Option<FastString>,
) -> ModuleGraph {
    let module_specifier = if let Some(code) = maybe_code {
        let specifier = ModuleSpecifier::parse("file:///src/index.ts").unwrap();

        emitter_factory.file_cache().insert(
            specifier.clone(),
            File {
                maybe_types: None,
                media_type: MediaType::TypeScript,
                source: code.as_str().into(),
                specifier: specifier.clone(),
                maybe_headers: None,
            },
        );

        specifier
    } else {
        let binding = std::fs::canonicalize(&file).unwrap();
        let specifier = binding.to_str().unwrap();
        let format_specifier = format!("file:///{}", specifier);

        ModuleSpecifier::parse(&format_specifier).unwrap()
    };

    let builder = ModuleGraphBuilder::new(emitter_factory, false);

    let create_module_graph_task = builder.create_graph_and_maybe_check(vec![module_specifier]);
    create_module_graph_task.await.unwrap()
}

pub async fn create_graph_from_specifiers(
    specifiers: Vec<ModuleSpecifier>,
    _is_dynamic: bool,
    maybe_emitter_factory: Arc<EmitterFactory>,
) -> Result<ModuleGraph, AnyError> {
    let builder = ModuleGraphBuilder::new(maybe_emitter_factory, false);
    let create_module_graph_task = builder.create_graph_and_maybe_check(specifiers);
    create_module_graph_task.await
}
