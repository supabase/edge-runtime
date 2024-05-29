// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

use deno_ast::MediaType;
use deno_ast::ModuleSpecifier;
use deno_core::error::AnyError;
use deno_fs;
use sb_core::cache::node::NodeAnalysisCache;
use sb_core::util::fs::canonicalize_path_maybe_not_exists;
use sb_node::analyze::CjsCodeAnalyzer;
use sb_node::analyze::NodeCodeTranslator;
use sb_node::analyze::{CjsAnalysis as ExtNodeCjsAnalysis, CjsAnalysisExports, CliCjsAnalysis};

pub type CliNodeCodeTranslator = NodeCodeTranslator<CliCjsCodeAnalyzer>;

/// Resolves a specifier that is pointing into a node_modules folder.
///
/// Note: This should be called whenever getting the specifier from
/// a Module::External(module) reference because that module might
/// not be fully resolved at the time deno_graph is analyzing it
/// because the node_modules folder might not exist at that time.
pub fn resolve_specifier_into_node_modules(specifier: &ModuleSpecifier) -> ModuleSpecifier {
    specifier
        .to_file_path()
        .ok()
        // this path might not exist at the time the graph is being created
        // because the node_modules folder might not yet exist
        .and_then(|path| canonicalize_path_maybe_not_exists(&path).ok())
        .and_then(|path| ModuleSpecifier::from_file_path(path).ok())
        .unwrap_or_else(|| specifier.clone())
}

pub struct CliCjsCodeAnalyzer {
    cache: NodeAnalysisCache,
    fs: deno_fs::FileSystemRc,
}

impl CliCjsCodeAnalyzer {
    pub fn new(cache: NodeAnalysisCache, fs: deno_fs::FileSystemRc) -> Self {
        Self { cache, fs }
    }

    fn inner_cjs_analysis(
        &self,
        specifier: &ModuleSpecifier,
        source: &str,
    ) -> Result<CliCjsAnalysis, AnyError> {
        let source_hash = NodeAnalysisCache::compute_source_hash(source);
        if let Some(analysis) = self
            .cache
            .get_cjs_analysis(specifier.as_str(), &source_hash)
        {
            return Ok(analysis);
        }

        let media_type = MediaType::from_specifier(specifier);
        if media_type == MediaType::Json {
            return Ok(CliCjsAnalysis::Cjs {
                exports: vec![],
                reexports: vec![],
            });
        }

        let parsed_source = deno_ast::parse_program(deno_ast::ParseParams {
            specifier: specifier.clone(),
            text_info: deno_ast::SourceTextInfo::new(source.into()),
            media_type,
            capture_tokens: true,
            scope_analysis: false,
            maybe_syntax: None,
        })?;
        let analysis = if parsed_source.is_script() {
            let analysis = parsed_source.analyze_cjs();
            CliCjsAnalysis::Cjs {
                exports: analysis.exports,
                reexports: analysis.reexports,
            }
        } else {
            CliCjsAnalysis::Esm
        };
        self.cache
            .set_cjs_analysis(specifier.as_str(), &source_hash, &analysis);

        Ok(analysis)
    }
}

impl CjsCodeAnalyzer for CliCjsCodeAnalyzer {
    fn analyze_cjs(
        &self,
        specifier: &ModuleSpecifier,
        source: Option<String>,
    ) -> Result<ExtNodeCjsAnalysis, AnyError> {
        let source = match source {
            Some(source) => source,
            None => self
                .fs
                .read_text_file_sync(&specifier.to_file_path().unwrap(), None)?,
        };
        let analysis = self.inner_cjs_analysis(specifier, &source)?;
        match analysis {
            CliCjsAnalysis::Esm => Ok(ExtNodeCjsAnalysis::Esm(source)),
            CliCjsAnalysis::Cjs { exports, reexports } => {
                Ok(ExtNodeCjsAnalysis::Cjs(CjsAnalysisExports {
                    exports,
                    reexports,
                }))
            }
        }
    }
}
