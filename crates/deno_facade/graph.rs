use std::path::PathBuf;
use std::sync::Arc;

use deno::deno_graph::ModuleGraph;
use deno::file_fetcher::File;
use deno::graph_util::ModuleGraphBuilder;
use deno_core::error::AnyError;
use deno_core::FastString;
use deno_core::ModuleSpecifier;
use eszip::EszipV2;

use crate::emitter::EmitterFactory;

#[allow(clippy::arc_with_non_send_sync)]
pub async fn create_eszip_from_graph_raw(
  graph: ModuleGraph,
  emitter_factory: Option<Arc<EmitterFactory>>,
) -> Result<EszipV2, AnyError> {
  // let emitter =
  //   emitter_factory.unwrap_or_else(|| Arc::new(EmitterFactory::new()));
  // let parser_arc = emitter.clone().parsed_source_cache().unwrap();
  // let parser = parser_arc.as_capturing_parser();
  // let transpile_options = emitter.transpile_options();
  // let emit_options = emitter.emit_options();

  // eszip::EszipV2::from_graph(eszip::FromGraphOptions {
  //   graph,
  //   parser,
  //   transpile_options,
  //   emit_options,
  //   relative_file_base: None,
  //   npm_packages: None,
  // })

  todo!()
}

pub async fn create_graph(
  file: PathBuf,
  emitter_factory: Arc<EmitterFactory>,
  maybe_code: &Option<FastString>,
) -> Result<ModuleGraph, AnyError> {
  // let module_specifier = if let Some(code) = maybe_code {
  //   let specifier = ModuleSpecifier::parse("file:///src/index.ts").unwrap();

  //   emitter_factory.file_fetcher()?.insert_memory_files(File {
  //     specifier: specifier.clone(),
  //     maybe_headers: None,
  //     source: code.as_bytes().into(),
  //   });

  //   specifier
  // } else {
  //   let binding =
  //     std::fs::canonicalize(&file).context("failed to read path")?;
  //   let specifier =
  //     binding.to_str().context("failed to convert path to str")?;
  //   let format_specifier = format!("file:///{}", specifier);

  //   ModuleSpecifier::parse(&format_specifier)
  //     .context("failed to parse specifier")?
  // };

  // let builder = ModuleGraphBuilder::new(emitter_factory, false);
  // let create_module_graph_task =
  //   builder.create_graph_and_maybe_check(vec![module_specifier]);

  // create_module_graph_task
  //   .await
  //   .context("failed to create the graph")

  todo!()
}
