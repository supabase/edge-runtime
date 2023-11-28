use deno_ast::{MediaType, ParseParams, SourceTextInfo};
use deno_core::error::AnyError;
use deno_core::{ExtensionFileSource, ExtensionFileSourceCode};
use std::path::Path;

pub fn maybe_transpile_source(
    source: &mut ExtensionFileSource,
) -> Result<&mut ExtensionFileSource, AnyError> {
    let media_type = if source.specifier.starts_with("node:") {
        MediaType::TypeScript
    } else {
        MediaType::from_path(Path::new(&source.specifier))
    };

    match media_type {
        MediaType::TypeScript => {}
        MediaType::JavaScript => return Ok(source),
        MediaType::Mjs => return Ok(source),
        _ => panic!(
            "Unsupported media type for snapshotting {media_type:?} for file {}",
            source.specifier
        ),
    }
    let code = source.load()?;

    let parsed = deno_ast::parse_module(ParseParams {
        specifier: source.specifier.to_string(),
        text_info: SourceTextInfo::from_string(code.as_str().to_owned()),
        media_type,
        capture_tokens: false,
        scope_analysis: false,
        maybe_syntax: None,
    })?;
    let transpiled_source = parsed.transpile(&deno_ast::EmitOptions {
        imports_not_used_as_values: deno_ast::ImportsNotUsedAsValues::Remove,
        inline_source_map: false,
        ..Default::default()
    })?;

    source.code = ExtensionFileSourceCode::Computed(transpiled_source.text.into());
    Ok(source)
}
