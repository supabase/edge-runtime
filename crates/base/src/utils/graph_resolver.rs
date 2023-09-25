use anyhow::bail;
use deno_core::error::AnyError;
use deno_core::ModuleSpecifier;
use eszip::deno_graph;
use eszip::deno_graph::source::{Resolver, DEFAULT_JSX_IMPORT_SOURCE_MODULE};
use import_map::ImportMap;
use module_fetcher::args::package_json::PackageJsonDepsProvider;
use module_fetcher::npm::{CliNpmRegistryApi, NpmResolution, PackageJsonDepsInstaller};
use module_fetcher::util::sync::AtomicFlag;
use std::sync::Arc;

/// Result of checking if a specifier is mapped via
/// an import map or package.json.
pub enum MappedResolution {
    None,
    PackageJson(ModuleSpecifier),
    ImportMap(ModuleSpecifier),
}

impl MappedResolution {
    pub fn into_specifier(self) -> Option<ModuleSpecifier> {
        match self {
            MappedResolution::None => Option::None,
            MappedResolution::PackageJson(specifier) => Some(specifier),
            MappedResolution::ImportMap(specifier) => Some(specifier),
        }
    }
}

/// Resolver for specifiers that could be mapped via an
/// import map or package.json.
#[derive(Debug)]
pub struct MappedSpecifierResolver {
    maybe_import_map: Option<Arc<ImportMap>>,
    package_json_deps_provider: Arc<PackageJsonDepsProvider>,
}

/// A resolver that takes care of resolution, taking into account loaded
/// import map, JSX settings.
#[derive(Debug)]
pub struct CliGraphResolver {
    mapped_specifier_resolver: MappedSpecifierResolver,
    maybe_default_jsx_import_source: Option<String>,
    maybe_jsx_import_source_module: Option<String>,
    maybe_vendor_specifier: Option<ModuleSpecifier>,
    no_npm: bool,
    npm_registry_api: Arc<CliNpmRegistryApi>,
    npm_resolution: Arc<NpmResolution>,
    package_json_deps_installer: Arc<PackageJsonDepsInstaller>,
    found_package_json_dep_flag: Arc<AtomicFlag>,
}

impl Default for CliGraphResolver {
    fn default() -> Self {
        // This is not ideal, but necessary for the LSP. In the future, we should
        // refactor the LSP and force this to be initialized.
        let npm_registry_api = Arc::new(CliNpmRegistryApi::new_uninitialized());
        let npm_resolution = Arc::new(NpmResolution::from_serialized(
            npm_registry_api.clone(),
            None,
            None,
        ));
        Self {
            mapped_specifier_resolver: MappedSpecifierResolver {
                maybe_import_map: Default::default(),
                package_json_deps_provider: Default::default(),
            },
            maybe_default_jsx_import_source: None,
            maybe_jsx_import_source_module: None,
            maybe_vendor_specifier: None,
            no_npm: false,
            npm_registry_api,
            npm_resolution,
            package_json_deps_installer: Default::default(),
            found_package_json_dep_flag: Default::default(),
        }
    }
}

impl Resolver for CliGraphResolver {
    fn default_jsx_import_source(&self) -> Option<String> {
        self.maybe_default_jsx_import_source.clone()
    }

    fn jsx_import_source_module(&self) -> &str {
        self.maybe_jsx_import_source_module
            .as_deref()
            .unwrap_or(DEFAULT_JSX_IMPORT_SOURCE_MODULE)
    }

    fn resolve(
        &self,
        specifier: &str,
        referrer: &ModuleSpecifier,
    ) -> Result<ModuleSpecifier, AnyError> {
        use MappedResolution::*;
        let result = match self
            .mapped_specifier_resolver
            .resolve(specifier, referrer)?
        {
            ImportMap(specifier) => Ok(specifier),
            PackageJson(specifier) => {
                // found a specifier in the package.json, so mark that
                // we need to do an "npm install" later
                self.found_package_json_dep_flag.raise();
                Ok(specifier)
            }
            None => deno_graph::resolve_import(specifier, referrer).map_err(|err| err.into()),
        };

        // When the user is vendoring, don't allow them to import directly from the vendor/ directory
        // as it might cause them confusion or duplicate dependencies. Additionally, this folder has
        // special treatment in the language server so it will definitely cause issues/confusion there
        // if they do this.
        if let Some(vendor_specifier) = &self.maybe_vendor_specifier {
            if let Ok(specifier) = &result {
                if specifier.as_str().starts_with(vendor_specifier.as_str()) {
                    bail!("Importing from the vendor directory is not permitted. Use a remote specifier instead or disable vendoring.");
                }
            }
        }

        result
    }
}
