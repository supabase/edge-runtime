use anyhow::anyhow;
use deno_config::JsxImportSourceConfig;
use deno_core::error::AnyError;
use deno_core::futures::future::LocalBoxFuture;
use deno_core::futures::FutureExt;
use deno_core::ModuleSpecifier;
use deno_npm::registry::NpmRegistryApi;
use deno_semver::package::PackageReq;
use eszip::deno_graph;
use eszip::deno_graph::source::{
    NpmResolver, ResolutionMode, ResolveError, Resolver, UnknownBuiltInNodeModuleError,
    DEFAULT_JSX_IMPORT_SOURCE_MODULE,
};
use eszip::deno_graph::NpmPackageReqResolution;
use import_map::ImportMap;
use log::debug;
use sb_core::util::sync::AtomicFlag;
use sb_node::is_builtin_node_module;
use sb_npm::installer::PackageJsonDepsInstaller;
use sb_npm::package_json::{PackageJsonDeps, PackageJsonDepsProvider};
use sb_npm::registry::CliNpmRegistryApi;
use sb_npm::{CliNpmResolver, InnerCliNpmResolverRef};
use std::path::PathBuf;
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
            MappedResolution::None => None,
            MappedResolution::PackageJson(specifier) => Some(specifier),
            MappedResolution::ImportMap(specifier) => Some(specifier),
        }
    }
}

fn resolve_package_json_dep(
    specifier: &str,
    deps: &PackageJsonDeps,
) -> Result<Option<ModuleSpecifier>, AnyError> {
    for (bare_specifier, req_result) in deps {
        if specifier.starts_with(bare_specifier) {
            let path = &specifier[bare_specifier.len()..];
            if path.is_empty() || path.starts_with('/') {
                let req = req_result
                    .as_ref()
                    .map_err(|_err| anyhow!("Parsing version constraints in the application-level package.json is more strict at the moment."))?;

                return Ok(Some(ModuleSpecifier::parse(&format!("npm:{req}{path}"))?));
            }
        }
    }

    Ok(None)
}

/// Resolver for specifiers that could be mapped via an
/// import map or package.json.
#[derive(Debug)]
pub struct MappedSpecifierResolver {
    maybe_import_map: Option<Arc<ImportMap>>,
    package_json_deps_provider: Arc<PackageJsonDepsProvider>,
}

impl MappedSpecifierResolver {
    pub fn new(
        maybe_import_map: Option<Arc<ImportMap>>,
        package_json_deps_provider: Arc<PackageJsonDepsProvider>,
    ) -> Self {
        Self {
            maybe_import_map,
            package_json_deps_provider,
        }
    }

    pub fn resolve(
        &self,
        specifier: &str,
        referrer: &ModuleSpecifier,
    ) -> Result<MappedResolution, AnyError> {
        // attempt to resolve with the import map first
        let maybe_import_map_err = match self
            .maybe_import_map
            .as_ref()
            .map(|import_map| import_map.resolve(specifier, referrer))
        {
            Some(Ok(value)) => return Ok(MappedResolution::ImportMap(value)),
            Some(Err(err)) => Some(err),
            None => None,
        };

        // then with package.json
        if let Some(deps) = self.package_json_deps_provider.deps() {
            if let Some(specifier) = resolve_package_json_dep(specifier, deps)? {
                return Ok(MappedResolution::PackageJson(specifier));
            }
        }

        // otherwise, surface the import map error or try resolving when has no import map
        if let Some(err) = maybe_import_map_err {
            Err(err.into())
        } else {
            Ok(MappedResolution::None)
        }
    }
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
    package_json_deps_installer: Arc<PackageJsonDepsInstaller>,
    found_package_json_dep_flag: Arc<AtomicFlag>,
    npm_resolver: Option<Arc<dyn CliNpmResolver>>,
}

#[derive(Default)]
pub struct CliGraphResolverOptions<'a> {
    pub maybe_jsx_import_source_config: Option<JsxImportSourceConfig>,
    pub maybe_import_map: Option<Arc<ImportMap>>,
    pub maybe_vendor_dir: Option<&'a PathBuf>,
    pub no_npm: bool,
}

impl CliGraphResolver {
    pub fn new(
        npm_registry_api: Arc<CliNpmRegistryApi>,
        package_json_deps_provider: Arc<PackageJsonDepsProvider>,
        package_json_deps_installer: Arc<PackageJsonDepsInstaller>,
        options: CliGraphResolverOptions,
        npm_resolver: Option<Arc<dyn CliNpmResolver>>,
    ) -> Self {
        Self {
            mapped_specifier_resolver: MappedSpecifierResolver {
                maybe_import_map: options.maybe_import_map,
                package_json_deps_provider,
            },
            maybe_default_jsx_import_source: options
                .maybe_jsx_import_source_config
                .as_ref()
                .and_then(|c| c.default_specifier.clone()),
            maybe_jsx_import_source_module: options
                .maybe_jsx_import_source_config
                .map(|c| c.module),
            maybe_vendor_specifier: options
                .maybe_vendor_dir
                .and_then(|v| ModuleSpecifier::from_directory_path(v).ok()),
            no_npm: options.no_npm,
            npm_registry_api,
            package_json_deps_installer,
            found_package_json_dep_flag: Default::default(),
            npm_resolver,
        }
    }

    pub fn set_jsx_import_source(&mut self, config: JsxImportSourceConfig) {
        self.maybe_jsx_import_source_module = Some(config.module);
        self.maybe_default_jsx_import_source = config.default_specifier;
    }

    pub fn as_graph_resolver(&self) -> &dyn Resolver {
        self
    }

    pub fn as_graph_npm_resolver(&self) -> &dyn NpmResolver {
        self
    }

    pub fn found_package_json_dep(&self) -> bool {
        self.found_package_json_dep_flag.is_raised()
    }

    pub async fn force_top_level_package_json_install(&self) -> Result<(), AnyError> {
        self.package_json_deps_installer
            .ensure_top_level_install()
            .await
    }

    pub async fn top_level_package_json_install_if_necessary(&self) -> Result<(), AnyError> {
        if self.found_package_json_dep_flag.is_raised() {
            self.force_top_level_package_json_install().await?;
        }
        Ok(())
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
        referrer_range: &deno_graph::Range,
        _mode: ResolutionMode,
    ) -> Result<ModuleSpecifier, ResolveError> {
        let referrer = &referrer_range.specifier;
        let result = self
            .mapped_specifier_resolver
            .resolve(specifier, referrer)
            .map_err(|err| err.into())
            .and_then(|resolution| match resolution {
                MappedResolution::ImportMap(specifier) => Ok(specifier),
                MappedResolution::PackageJson(specifier) => {
                    // found a specifier in the package.json, so mark that
                    // we need to do an "npm install" later
                    self.found_package_json_dep_flag.raise();
                    Ok(specifier)
                }
                MappedResolution::None => {
                    deno_graph::resolve_import(specifier, &referrer_range.specifier)
                        .map_err(|err| err.into())
                }
            });

        // When the user is vendoring, don't allow them to import directly from the vendor/ directory
        // as it might cause them confusion or duplicate dependencies. Additionally, this folder has
        // special treatment in the language server so it will definitely cause issues/confusion there
        // if they do this.
        if let Some(vendor_specifier) = &self.maybe_vendor_specifier {
            if let Ok(specifier) = &result {
                if specifier.as_str().starts_with(vendor_specifier.as_str()) {
                    return Err(ResolveError::Other(anyhow!(
                        "Importing from the vendor directory is not permitted. Use a remote specifier instead or disable vendoring."
                    )));
                }
            }
        }

        result
    }
}

impl NpmResolver for CliGraphResolver {
    fn resolve_builtin_node_module(
        &self,
        specifier: &ModuleSpecifier,
    ) -> Result<Option<String>, UnknownBuiltInNodeModuleError> {
        if specifier.scheme() != "node" {
            return Ok(None);
        }

        let module_name = specifier.path().to_string();
        if is_builtin_node_module(&module_name) {
            Ok(Some(module_name))
        } else {
            Err(UnknownBuiltInNodeModuleError { module_name })
        }
    }

    fn on_resolve_bare_builtin_node_module(&self, module_name: &str, range: &deno_graph::Range) {
        let deno_graph::Range {
            start, specifier, ..
        } = range.clone();
        let line = start.line + 1;
        let column = start.character + 1;
        debug!("Warning: Resolving \"{module_name}\" as \"node:{module_name}\" at {specifier}:{line}:{column}. If you want to use a built-in Node module, add a \"node:\" prefix.")
    }

    fn load_and_cache_npm_package_info(
        &self,
        package_name: &str,
    ) -> LocalBoxFuture<'static, Result<(), AnyError>> {
        if self.no_npm {
            // return it succeeded and error at the import site below
            return Box::pin(deno_core::futures::future::ready(Ok(())));
        }
        // this will internally cache the package information
        let package_name = package_name.to_string();
        let api = self.npm_registry_api.clone();

        async move {
            api.package_info(&package_name)
                .await
                .map(|_| ())
                .map_err(|err| err.into())
        }
        .boxed()
    }

    fn resolve_npm(&self, package_req: &PackageReq) -> NpmPackageReqResolution {
        match &self.npm_resolver {
            Some(npm_resolver) => match npm_resolver.as_inner() {
                InnerCliNpmResolverRef::Managed(npm_resolver) => {
                    npm_resolver.resolve_npm_for_deno_graph(package_req)
                }
                // if we are using byonm, then this should never be called because
                // we don't use deno_graph's npm resolution in this case
                InnerCliNpmResolverRef::Byonm(_) => unreachable!(),
            },
            None => NpmPackageReqResolution::Err(anyhow!(
                "npm specifiers were requested; but --no-npm is specified"
            )),
        }
    }
}
