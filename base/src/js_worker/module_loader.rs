use anyhow::{bail, Error};
use deno_ast::EmitOptions;
use deno_ast::MediaType;
use deno_ast::ParseParams;
use deno_ast::SourceTextInfo;
use deno_core::futures::FutureExt;
use deno_core::ModuleLoader;
use deno_core::ModuleSource;
use deno_core::ModuleSourceFuture;
use deno_core::ModuleSpecifier;
use deno_core::ModuleType;
use deno_core::ResolutionKind;
use std::pin::Pin;

struct ModuleTypeResult {
    media_type: MediaType,
    module_type: ModuleType,
    should_transpile: bool,
}

fn get_module_type(
    module_specifier: ModuleSpecifier,
    content_type: &str,
) -> Result<ModuleTypeResult, Error> {
    let media_type = MediaType::from_content_type(&module_specifier, content_type);
    let (module_type, should_transpile) = match media_type {
        MediaType::JavaScript | MediaType::Mjs | MediaType::Cjs => (ModuleType::JavaScript, false),
        MediaType::Jsx => (ModuleType::JavaScript, true),
        MediaType::TypeScript
        | MediaType::Mts
        | MediaType::Cts
        | MediaType::Dts
        | MediaType::Dmts
        | MediaType::Dcts
        | MediaType::Tsx => (ModuleType::JavaScript, true),
        MediaType::Json => (ModuleType::Json, false),
        _ => bail!(
            "{:?} module type not supported (specifier {:?})",
            media_type,
            module_specifier.as_str()
        ),
    };

    Ok(ModuleTypeResult {
        media_type,
        module_type,
        should_transpile,
    })
}

pub struct DefaultModuleLoader;

impl ModuleLoader for DefaultModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, Error> {
        Ok(deno_core::resolve_import(specifier, referrer)?)
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<ModuleSpecifier>,
        _is_dyn_import: bool,
    ) -> Pin<Box<ModuleSourceFuture>> {
        let module_specifier = module_specifier.clone();
        async move {
            let (media_type, module_type, code, should_transpile) = match module_specifier.scheme()
            {
                "file" => {
                    let content_type = "text/plain";
                    let ModuleTypeResult {
                        media_type,
                        module_type,
                        should_transpile,
                    } = get_module_type(module_specifier.clone(), &content_type)?;
                    let path = module_specifier.to_file_path().unwrap();
                    let code = std::fs::read_to_string(&path)?;
                    (media_type, module_type, code, should_transpile)
                }
                "http" | "https" => {
                    let res = reqwest::get(module_specifier.clone()).await?;
                    let content_type = res
                        .headers()
                        .get("content-type")
                        .map(|v| v.to_str())
                        .unwrap()?;
                    let ModuleTypeResult {
                        media_type,
                        module_type,
                        should_transpile,
                    } = get_module_type(module_specifier.clone(), content_type)?;
                    let code = res.text().await?;
                    (media_type, module_type, code, should_transpile)
                }
                _ => {
                    bail!(
                        "unsupported module url scheme: {:?}",
                        module_specifier.scheme()
                    )
                }
            };

            let code = if should_transpile {
                let parsed = deno_ast::parse_module(ParseParams {
                    specifier: module_specifier.to_string(),
                    text_info: SourceTextInfo::from_string(code),
                    media_type,
                    capture_tokens: false,
                    scope_analysis: false,
                    maybe_syntax: None,
                })?;
                parsed
                    .transpile(&EmitOptions {
                        inline_source_map: true,
                        inline_sources: true,
                        source_map: true,
                        ..Default::default()
                    })?
                    .text
            } else {
                code
            };

            let module = ModuleSource {
                code: code.into_bytes().into_boxed_slice(),
                module_type,
                module_url_specified: module_specifier.to_string(),
                module_url_found: module_specifier.to_string(),
            };
            Ok(module)
        }
        .boxed_local()
    }
}
