use crate::errors_rt::get_error_class_name;
use crate::js_worker::emitter::EmitterFactory;
use crate::utils::graph_util::deno_graph::ModuleError;
use crate::utils::graph_util::deno_graph::ResolutionError;
use deno_core::error::{custom_error, AnyError};
use deno_core::ModuleSpecifier;
use eszip::deno_graph;
use eszip::deno_graph::{ModuleGraph, ModuleGraphError};
use std::path::PathBuf;

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

pub async fn build_graph_with_npm_resolution<'a>(
    graph: &mut ModuleGraph,
    roots: Vec<ModuleSpecifier>,
    loader: &mut dyn deno_graph::source::Loader,
    options: deno_graph::BuildOptions<'a>,
) -> Result<(), AnyError> {
    //TODO: NPM resolvers

    // ensure an "npm install" is done if the user has explicitly
    // opted into using a node_modules directory
    // if self.options.node_modules_dir_enablement() == Some(true) {
    //     self.resolver.force_top_level_package_json_install().await?;
    // }

    graph.build(roots, loader, options).await;

    // ensure that the top level package.json is installed if a
    // specifier was matched in the package.json
    // self
    //     .resolver
    //     .top_level_package_json_install_if_necessary()
    //     .await?;

    // resolve the dependencies of any pending dependencies
    // that were inserted by building the graph
    // self.npm_resolver.resolve_pending().await?;

    Ok(())
}

pub async fn create_graph_and_maybe_check(
    roots: Vec<ModuleSpecifier>,
) -> Result<deno_graph::ModuleGraph, AnyError> {
    let emitter_factory = EmitterFactory::new();

    let mut cache = emitter_factory.file_fetcher_loader();
    let analyzer = emitter_factory.parsed_source_cache().unwrap().as_analyzer();
    let graph_kind = deno_graph::GraphKind::CodeOnly;
    let mut graph = ModuleGraph::new(graph_kind);
    let graph_resolver = emitter_factory.graph_resolver();

    build_graph_with_npm_resolution(
        &mut graph,
        roots,
        cache.as_mut(),
        deno_graph::BuildOptions {
            is_dynamic: false,
            imports: vec![],
            resolver: Some(&*graph_resolver),
            npm_resolver: None,
            module_analyzer: Some(&*analyzer),
            reporter: None,
        },
    )
    .await?;

    //let graph = Arc::new(graph);
    // graph_valid_with_cli_options(&graph, &graph.roots, &self.options)?;
    // if let Some(lockfile) = &self.lockfile {
    //     graph_lock_or_exit(&graph, &mut lockfile.lock());
    // }

    // if self.options.type_check_mode().is_true() {
    //     self
    //         .type_checker
    //         .check(
    //             graph.clone(),
    //             check::CheckOptions {
    //                 lib: self.options.ts_type_lib_window(),
    //                 log_ignored_options: true,
    //                 reload: true, // TODO: ?
    //             },
    //         )
    //         .await?;
    // }

    Ok(graph)
}

pub async fn create_module_graph_from_path(
    main_service_path: &str,
) -> Result<ModuleGraph, Box<dyn std::error::Error>> {
    let index = PathBuf::from(main_service_path);
    let binding = std::fs::canonicalize(&index)?;
    let specifier = binding.to_str().ok_or("Failed to convert path to string")?;
    let format_specifier = format!("file:///{}", specifier);
    let module_specifier = ModuleSpecifier::parse(&format_specifier)?;
    let graph = create_graph_and_maybe_check(vec![module_specifier])
        .await
        .unwrap();
    Ok(graph)
}

pub async fn create_eszip_from_graph(graph: ModuleGraph) -> Vec<u8> {
    let emitter = EmitterFactory::new();
    let parser_arc = emitter.parsed_source_cache().unwrap();
    let parser = parser_arc.as_capturing_parser();

    let eszip = eszip::EszipV2::from_graph(graph, &parser, Default::default());

    eszip.unwrap().into_bytes()
}
