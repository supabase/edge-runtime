use crate::js_worker::emitter::EmitterFactory;
use crate::js_worker::node_module_loader::NpmModuleLoader;
use crate::utils::graph_resolver::MappedSpecifierResolver;
use deno_ast::MediaType;
use deno_core::error::{generic_error, type_error, AnyError};
use deno_core::futures::FutureExt;
use deno_core::{ModuleLoader, ModuleSpecifier, ModuleType, ResolutionKind};
use deno_semver::npm::NpmPackageReqReference;
use eszip::EszipV2;
use import_map::ImportMap;
use module_fetcher::args::lockfile::Lockfile;
use module_fetcher::args::package_json::PackageJsonDepsProvider;
use module_fetcher::file_fetcher::get_source_from_data_url;
use sb_eszip::module_loader::EszipPayloadKind;
use sb_node::NodePermissions;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

pub struct SharedModuleLoaderState {
    eszip: eszip::EszipV2,
    mapped_specifier_resolver: MappedSpecifierResolver,
    npm_module_loader: Arc<NpmModuleLoader>,
}

#[derive(Clone)]
pub struct EmbeddedModuleLoader {
    shared: Arc<SharedModuleLoaderState>,
}

fn arc_u8_to_arc_str(arc_u8: Arc<[u8]>) -> Result<Arc<str>, std::str::Utf8Error> {
    // Check that the string is valid UTF-8.
    std::str::from_utf8(&arc_u8)?;
    // SAFETY: the string is valid UTF-8, and the layout Arc<[u8]> is the same as
    // Arc<str>. This is proven by the From<Arc<str>> impl for Arc<[u8]> from the
    // standard library.
    Ok(unsafe { std::mem::transmute(arc_u8) })
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

        let permissions: Arc<dyn NodePermissions> = Arc::new(sb_node::AllowAllNodePermissions);
        if let Some(result) = self.shared.npm_module_loader.resolve_if_in_npm_package(
            specifier,
            &referrer,
            &*permissions,
        ) {
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
            return self
                .shared
                .npm_module_loader
                .resolve_req_reference(&reference, &*permissions);
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
        is_dynamic: bool,
    ) -> Pin<Box<deno_core::ModuleSourceFuture>> {
        let is_data_uri = get_source_from_data_url(original_specifier).ok();
        if let Some((source, _)) = is_data_uri {
            return Box::pin(deno_core::futures::future::ready(Ok(
                deno_core::ModuleSource::new(
                    deno_core::ModuleType::JavaScript,
                    source.into(),
                    original_specifier,
                ),
            )));
        }

        let permissions: Arc<dyn NodePermissions> = Arc::new(sb_node::AllowAllNodePermissions);
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
                        code_source.code,
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
                code.into(),
                &original_specifier,
                &found_specifier,
            ))
        }
        .boxed_local()
    }
}

pub fn create_shared_state_for_module_loader(
    mut eszip: eszip::EszipV2,
    maybe_import_map: Option<Arc<ImportMap>>,
) -> SharedModuleLoaderState {
    let mut emitter = EmitterFactory::new();

    if let Some(snapshot) = eszip.take_npm_snapshot() {
        println!("snapshot saved");

        println!(
            "{}",
            deno_core::serde_json::to_value(snapshot.clone().into_serialized().packages)
                .unwrap()
                .to_string()
        );
        emitter.set_npm_snapshot(Some(snapshot));
    }

    let shared_module_state = SharedModuleLoaderState {
        eszip,
        mapped_specifier_resolver: MappedSpecifierResolver::new(
            maybe_import_map,
            emitter.package_json_deps_provider(),
        ),
        npm_module_loader: emitter.npm_module_loader(),
    };
    shared_module_state
}

pub async fn create_module_loader_for_standalone_from_eszip_kind(
    eszip_payload_kind: EszipPayloadKind,
    maybe_import_map: Option<Arc<ImportMap>>,
) -> EmbeddedModuleLoader {
    use deno_core::futures::io::{AllowStdIo, BufReader};
    let bytes = match eszip_payload_kind {
        EszipPayloadKind::JsBufferKind(js_buffer) => Vec::from(&*js_buffer),
        EszipPayloadKind::VecKind(vec) => vec,
    };

    let bufreader = BufReader::new(AllowStdIo::new(bytes.as_slice()));
    let (eszip, loader) = eszip::EszipV2::parse(bufreader).await.unwrap();

    loader.await.unwrap();

    EmbeddedModuleLoader {
        shared: Arc::new(create_shared_state_for_module_loader(
            eszip,
            maybe_import_map,
        )),
    }
}
