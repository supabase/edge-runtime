use std::borrow::Cow;
use sb_core::cache::node::NodeAnalysisCache;
use sb_node::analyze::CjsAnalysis as ExtNodeCjsAnalysis;
use sb_node::analyze::CjsCodeAnalyzer;
use sb_node::analyze::NodeCodeTranslator;
use deno_ast::ModuleSpecifier;
use deno_ast::CjsAnalysis;
use deno_core::error::AnyError;
use deno_ast::MediaType;

pub type CliNodeCodeTranslator = NodeCodeTranslator<CliCjsCodeAnalyzer>;

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
    ) -> Result<CjsAnalysis, AnyError> {
        let source_hash = NodeAnalysisCache::compute_source_hash(source);
        if let Some(analysis) = self
            .cache
            .get_cjs_analysis(specifier.as_str(), &source_hash)
        {
            return Ok(analysis);
        }

        let media_type = MediaType::from_specifier(specifier);
        if media_type == MediaType::Json {
            return Ok(CjsAnalysis {
                exports: vec![],
                reexports: vec![],
            });
        }

        let parsed_source = deno_ast::parse_script(deno_ast::ParseParams {
            specifier: specifier.to_string(),
            text_info: deno_ast::SourceTextInfo::new(source.into()),
            media_type,
            capture_tokens: true,
            scope_analysis: false,
            maybe_syntax: None,
        })?;
        let analysis = parsed_source.analyze_cjs();
        self.cache
            .set_cjs_analysis(specifier.as_str(), &source_hash, &analysis);

        Ok(analysis)
    }
}

impl CjsCodeAnalyzer for CliCjsCodeAnalyzer {
    fn analyze_cjs(
        &self,
        specifier: &ModuleSpecifier,
        source: Option<&str>,
    ) -> Result<ExtNodeCjsAnalysis, AnyError> {
        let source = match source {
            Some(source) => Cow::Borrowed(source),
            None => Cow::Owned(
                self.fs
                    .read_text_file_sync(&specifier.to_file_path().unwrap())?,
            ),
        };
        let analysis = self.inner_cjs_analysis(specifier, &source)?;
        Ok(ExtNodeCjsAnalysis {
            exports: analysis.exports,
            reexports: analysis.reexports,
        })
    }
}