// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

use crate::node::node_module_loader::NpmModuleLoader;
use deno_ast::MediaType;
use deno_core::error::generic_error;
use deno_core::error::type_error;
use deno_core::error::AnyError;
use deno_core::futures::FutureExt;
use deno_core::ModuleSpecifier;
use deno_core::ModuleType;
use deno_core::ResolutionKind;
use deno_core::{ModuleLoader, ModuleSourceCode};
use deno_semver::npm::NpmPackageReqReference;
use sb_core::file_fetcher::get_source_from_data_url;
use std::pin::Pin;
use std::sync::Arc;

use crate::node::cli_node_resolver::CliNodeResolver;
use crate::util::arc_u8_to_arc_str;
use sb_graph::graph_resolver::MappedSpecifierResolver;

pub struct SharedModuleLoaderState {
    pub(crate) eszip: eszip::EszipV2,
    pub(crate) mapped_specifier_resolver: MappedSpecifierResolver,
    pub(crate) npm_module_loader: Arc<NpmModuleLoader>,
    pub(crate) node_resolver: Arc<CliNodeResolver>,
}

#[derive(Clone)]
pub struct EmbeddedModuleLoader {
    pub(crate) shared: Arc<SharedModuleLoaderState>,
}

impl ModuleLoader for EmbeddedModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, AnyError> {
        let referrer = if referrer == "." {
            if kind != ResolutionKind::MainModule {
                return Err(generic_error(format!(
                    "Expected to resolve main module, got {:?} instead.",
                    kind
                )));
            }
            let current_dir = std::env::current_dir().unwrap();
            deno_core::resolve_path(".", &current_dir)?
        } else {
            ModuleSpecifier::parse(referrer)
                .map_err(|err| type_error(format!("Referrer uses invalid specifier: {}", err)))?
        };

        let permissions = sb_node::allow_all();
        if let Some(result) =
            self.shared
                .node_resolver
                .resolve_if_in_npm_package(specifier, &referrer, &*permissions)
        {
            return result;
        }

        let maybe_mapped = self
            .shared
            .mapped_specifier_resolver
            .resolve(specifier, &referrer)?
            .into_specifier();

        // npm specifier
        let specifier_text = maybe_mapped
            .as_ref()
            .map(|r| r.as_str())
            .unwrap_or(specifier);
        if let Ok(reference) = NpmPackageReqReference::from_str(specifier_text) {
            return self.shared.node_resolver.resolve_req_reference(
                &reference,
                &*permissions,
                &referrer,
            );
        }

        match maybe_mapped {
            Some(resolved) => Ok(resolved),
            None => {
                deno_core::resolve_import(specifier, referrer.as_str()).map_err(|err| err.into())
            }
        }
    }

    fn load(
        &self,
        original_specifier: &ModuleSpecifier,
        maybe_referrer: Option<&ModuleSpecifier>,
        _is_dynamic: bool,
    ) -> Pin<Box<deno_core::ModuleSourceFuture>> {
        let is_data_uri = get_source_from_data_url(original_specifier).ok();
        let permissions = sb_node::allow_all();
        if let Some((source, _)) = is_data_uri {
            return Box::pin(deno_core::futures::future::ready(Ok(
                deno_core::ModuleSource::new(
                    deno_core::ModuleType::JavaScript,
                    ModuleSourceCode::String(source.into()),
                    original_specifier,
                ),
            )));
        }

        if let Some(result) = self.shared.npm_module_loader.load_sync_if_in_npm_package(
            original_specifier,
            maybe_referrer,
            &*permissions,
        ) {
            return match result {
                Ok(code_source) => Box::pin(deno_core::futures::future::ready(Ok(
                    deno_core::ModuleSource::new_with_redirect(
                        match code_source.media_type {
                            MediaType::Json => ModuleType::Json,
                            _ => ModuleType::JavaScript,
                        },
                        ModuleSourceCode::String(code_source.code),
                        original_specifier,
                        &code_source.found_url,
                    ),
                ))),
                Err(err) => Box::pin(deno_core::futures::future::ready(Err(err))),
            };
        }

        let Some(module) = self.shared.eszip.get_module(original_specifier.as_str()) else {
            return Box::pin(deno_core::futures::future::ready(Err(type_error(format!(
                "Module not found: {}",
                original_specifier
            )))));
        };
        let original_specifier = original_specifier.clone();
        let found_specifier =
            ModuleSpecifier::parse(&module.specifier).expect("invalid url in eszip");

        async move {
            let code = module
                .source()
                .await
                .ok_or_else(|| type_error(format!("Module not found: {}", original_specifier)))?;
            let code =
                arc_u8_to_arc_str(code).map_err(|_| type_error("Module source is not utf-8"))?;
            Ok(deno_core::ModuleSource::new_with_redirect(
                match module.kind {
                    eszip::ModuleKind::JavaScript => ModuleType::JavaScript,
                    eszip::ModuleKind::Json => ModuleType::Json,
                    eszip::ModuleKind::Jsonc => {
                        return Err(type_error("jsonc modules not supported"))
                    }
                    eszip::ModuleKind::OpaqueData => {
                        unreachable!();
                    }
                },
                ModuleSourceCode::String(code.into()),
                &original_specifier,
                &found_specifier,
            ))
        }
        .boxed_local()
    }
}
