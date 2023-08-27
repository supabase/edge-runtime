use anyhow::{bail, Error};
use deno_core::futures::io::{AllowStdIo, BufReader};
use deno_core::futures::FutureExt;
use deno_core::url::Url;
use deno_core::JsBuffer;
use deno_core::ModuleLoader;
use deno_core::ModuleSource;
use deno_core::ModuleSourceFuture;
use deno_core::ModuleSpecifier;
use deno_core::ResolutionKind;
use import_map::ImportMap;
use std::path::Path;
use std::pin::Pin;

pub struct EszipModuleLoader {
    eszip: eszip::EszipV2,
    maybe_import_map: Option<ImportMap>,
}

impl EszipModuleLoader {
    pub async fn new(bytes: JsBuffer, maybe_import_map: Option<ImportMap>) -> Result<Self, Error> {
        let bytes = Vec::from(&*bytes);

        let bufreader = BufReader::new(AllowStdIo::new(bytes.as_slice()));
        let (eszip, loader) = eszip::EszipV2::parse(bufreader).await?;

        loader.await?;

        Ok(Self {
            eszip,
            maybe_import_map,
        })
    }
}

impl ModuleLoader for EszipModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, Error> {
        if let Some(import_map) = &self.maybe_import_map {
            let referrer_relative = Path::new(referrer).is_relative();
            let referrer_url = if referrer_relative {
                import_map.base_url().join(referrer)
            } else {
                Url::parse(referrer)
            };
            if referrer_url.is_err() {
                return referrer_url.map_err(|err| err.into());
            }

            let referrer_url = referrer_url.unwrap();
            import_map
                .resolve(specifier, &referrer_url)
                .map_err(|err| err.into())
        } else {
            deno_core::resolve_import(specifier, referrer).map_err(|err| err.into())
        }
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<&ModuleSpecifier>,
        _is_dyn_import: bool,
    ) -> Pin<Box<ModuleSourceFuture>> {
        let maybe_module = self.eszip.get_module(module_specifier.as_str());
        let module_specifier = module_specifier.clone();

        async move {
            if let Some(module) = maybe_module {
                if let Some(code) = module.source().await {
                    let code = std::str::from_utf8(&code)?.to_string();
                    let module_type = match module.kind {
                        eszip::ModuleKind::JavaScript => Some(deno_core::ModuleType::JavaScript),
                        eszip::ModuleKind::Json => Some(deno_core::ModuleType::Json),
                        eszip::ModuleKind::Jsonc => None,
                        eszip::ModuleKind::OpaqueData => None,
                    };
                    if module_type.is_none() {
                        bail!("invalid module type {}", &module_specifier)
                    }
                    let module = ModuleSource::new_with_redirect(
                        module_type.unwrap(),
                        code.into(),
                        &module_specifier,
                        &Url::parse(&module.specifier)?,
                    );

                    Ok(module)
                } else {
                    bail!("module source already taken {}", &module_specifier)
                }
            } else {
                bail!("module not found {}", &module_specifier)
            }
        }
        .boxed_local()
    }
}
