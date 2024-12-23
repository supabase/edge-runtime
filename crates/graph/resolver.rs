use std::{
    borrow::Cow,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use dashmap::DashSet;
use deno_ast::MediaType;
use deno_config::{
    package_json::PackageJsonDepValue,
    workspace::{MappedResolution, MappedResolutionError, WorkspaceResolver},
    JsxImportSourceConfig,
};
use deno_core::{error::AnyError, unsync::AtomicFlag, ModuleSourceCode, ModuleSpecifier};
use deno_graph::{
    source::{
        ResolutionMode, ResolveError, Resolver, UnknownBuiltInNodeModuleError,
        DEFAULT_JSX_IMPORT_SOURCE_MODULE,
    },
    NpmLoadError, NpmResolvePkgReqsResult,
};
use deno_npm::resolution::NpmResolutionError;
use deno_semver::{npm::NpmPackageReqReference, package::PackageReq};
use npm::{byonm::ByonmCliNpmResolver, CliNpmResolver, InnerCliNpmResolverRef};
use sb_core::node::CliNodeCodeTranslator;
use sb_node::{
    errors::{ClosestPkgJsonError, UrlToNodeResolutionError},
    is_builtin_node_module, parse_npm_pkg_name, NodeModuleKind, NodeResolution, NodeResolutionMode,
    NodeResolver, NpmResolver, PackageJson,
};

pub struct ModuleCodeStringSource {
    pub code: ModuleSourceCode,
    pub found_url: ModuleSpecifier,
    pub media_type: MediaType,
}

#[derive(Debug)]
pub struct CliNodeResolver {
    cjs_resolutions: Arc<CjsResolutionStore>,
    fs: Arc<dyn deno_fs::FileSystem>,
    node_resolver: Arc<NodeResolver>,
    // todo(dsherret): remove this pub(crate)
    pub(crate) npm_resolver: Arc<dyn CliNpmResolver>,
}

impl CliNodeResolver {
    pub fn new(
        cjs_resolutions: Arc<CjsResolutionStore>,
        fs: Arc<dyn deno_fs::FileSystem>,
        node_resolver: Arc<NodeResolver>,
        npm_resolver: Arc<dyn CliNpmResolver>,
    ) -> Self {
        Self {
            cjs_resolutions,
            fs,
            node_resolver,
            npm_resolver,
        }
    }

    pub fn in_npm_package(&self, specifier: &ModuleSpecifier) -> bool {
        self.npm_resolver.in_npm_package(specifier)
    }

    pub fn get_closest_package_json(
        &self,
        referrer: &ModuleSpecifier,
    ) -> Result<Option<Arc<PackageJson>>, ClosestPkgJsonError> {
        self.node_resolver.get_closest_package_json(referrer)
    }

    pub fn resolve_if_in_npm_package(
        &self,
        specifier: &str,
        referrer: &ModuleSpecifier,
        mode: NodeResolutionMode,
    ) -> Option<Result<Option<NodeResolution>, AnyError>> {
        if self.in_npm_package(referrer) {
            // we're in an npm package, so use node resolution
            Some(self.resolve(specifier, referrer, mode))
        } else {
            None
        }
    }

    pub fn resolve(
        &self,
        specifier: &str,
        referrer: &ModuleSpecifier,
        mode: NodeResolutionMode,
    ) -> Result<Option<NodeResolution>, AnyError> {
        let referrer_kind = if self.cjs_resolutions.contains(referrer) {
            NodeModuleKind::Cjs
        } else {
            NodeModuleKind::Esm
        };

        self.handle_node_resolve_result(
            self.node_resolver
                .resolve(specifier, referrer, referrer_kind, mode)
                .map_err(AnyError::from),
        )
    }

    pub fn resolve_req_reference(
        &self,
        req_ref: &NpmPackageReqReference,
        referrer: &ModuleSpecifier,
        mode: NodeResolutionMode,
    ) -> Result<NodeResolution, AnyError> {
        self.resolve_req_with_sub_path(req_ref.req(), req_ref.sub_path(), referrer, mode)
    }

    pub fn resolve_req_with_sub_path(
        &self,
        req: &PackageReq,
        sub_path: Option<&str>,
        referrer: &ModuleSpecifier,
        mode: NodeResolutionMode,
    ) -> Result<NodeResolution, AnyError> {
        let package_folder = self
            .npm_resolver
            .resolve_pkg_folder_from_deno_module_req(req, referrer)?;
        let maybe_resolution = self.maybe_resolve_package_sub_path_from_deno_module(
            &package_folder,
            sub_path,
            Some(referrer),
            mode,
        )?;
        match maybe_resolution {
            Some(resolution) => Ok(resolution),
            None => {
                if self.npm_resolver.as_byonm().is_some() {
                    let package_json_path = package_folder.join("package.json");
                    if !self.fs.exists_sync(&package_json_path) {
                        return Err(anyhow!(
                            "Could not find '{}'. Deno expects the node_modules/ directory to be up to date. Did you forget to run `npm install`?",
                            package_json_path.display()
                            ));
                    }
                }
                Err(anyhow!(
                    "Failed resolving '{}{}' in '{}'.",
                    req,
                    sub_path.map(|s| format!("/{}", s)).unwrap_or_default(),
                    package_folder.display()
                ))
            }
        }
    }

    pub fn resolve_package_sub_path_from_deno_module(
        &self,
        package_folder: &Path,
        sub_path: Option<&str>,
        maybe_referrer: Option<&ModuleSpecifier>,
        mode: NodeResolutionMode,
    ) -> Result<NodeResolution, AnyError> {
        self.maybe_resolve_package_sub_path_from_deno_module(
            package_folder,
            sub_path,
            maybe_referrer,
            mode,
        )?
        .ok_or_else(|| {
            anyhow!(
                "Failed resolving '{}' in '{}'.",
                sub_path
                    .map(|s| format!("/{}", s))
                    .unwrap_or_else(|| ".".to_string()),
                package_folder.display(),
            )
        })
    }

    pub fn maybe_resolve_package_sub_path_from_deno_module(
        &self,
        package_folder: &Path,
        sub_path: Option<&str>,
        maybe_referrer: Option<&ModuleSpecifier>,
        mode: NodeResolutionMode,
    ) -> Result<Option<NodeResolution>, AnyError> {
        self.handle_node_resolve_result(
            self.node_resolver
                .resolve_package_subpath_from_deno_module(
                    package_folder,
                    sub_path,
                    maybe_referrer,
                    mode,
                )
                .map_err(AnyError::from),
        )
    }

    pub fn handle_if_in_node_modules(
        &self,
        specifier: ModuleSpecifier,
    ) -> Result<ModuleSpecifier, AnyError> {
        // skip canonicalizing if we definitely know it's unnecessary
        if specifier.scheme() == "file" && specifier.path().contains("/node_modules/") {
            // Specifiers in the node_modules directory are canonicalized
            // so canoncalize then check if it's in the node_modules directory.
            // If so, check if we need to store this specifier as being a CJS
            // resolution.
            let specifier = sb_core::node::resolve_specifier_into_node_modules(&specifier);
            if self.in_npm_package(&specifier) {
                let resolution = self.node_resolver.url_to_node_resolution(specifier)?;
                if let NodeResolution::CommonJs(specifier) = &resolution {
                    self.cjs_resolutions.insert(specifier.clone());
                }
                return Ok(resolution.into_url());
            }
        }

        Ok(specifier)
    }

    pub fn url_to_node_resolution(
        &self,
        specifier: ModuleSpecifier,
    ) -> Result<NodeResolution, UrlToNodeResolutionError> {
        self.node_resolver.url_to_node_resolution(specifier)
    }

    fn handle_node_resolve_result(
        &self,
        result: Result<Option<NodeResolution>, AnyError>,
    ) -> Result<Option<NodeResolution>, AnyError> {
        match result? {
            Some(response) => {
                if let NodeResolution::CommonJs(specifier) = &response {
                    // remember that this was a common js resolution
                    self.cjs_resolutions.insert(specifier.clone());
                }
                Ok(Some(response))
            }
            None => Ok(None),
        }
    }
}

#[derive(Clone)]
pub struct NpmModuleLoader {
    cjs_resolutions: Arc<CjsResolutionStore>,
    node_code_translator: Arc<CliNodeCodeTranslator>,
    fs: Arc<dyn deno_fs::FileSystem>,
    node_resolver: Arc<CliNodeResolver>,
}

impl NpmModuleLoader {
    pub fn new(
        cjs_resolutions: Arc<CjsResolutionStore>,
        node_code_translator: Arc<CliNodeCodeTranslator>,
        fs: Arc<dyn deno_fs::FileSystem>,
        node_resolver: Arc<CliNodeResolver>,
    ) -> Self {
        Self {
            cjs_resolutions,
            node_code_translator,
            fs,
            node_resolver,
        }
    }

    pub async fn load_if_in_npm_package(
        &self,
        specifier: &ModuleSpecifier,
        maybe_referrer: Option<&ModuleSpecifier>,
    ) -> Option<Result<ModuleCodeStringSource, AnyError>> {
        if self.node_resolver.in_npm_package(specifier) {
            Some(self.load(specifier, maybe_referrer).await)
        } else {
            None
        }
    }

    pub async fn load(
        &self,
        specifier: &ModuleSpecifier,
        maybe_referrer: Option<&ModuleSpecifier>,
    ) -> Result<ModuleCodeStringSource, AnyError> {
        let file_path = specifier.to_file_path().unwrap();
        let code = self
            .fs
            .read_file_async(file_path.clone(), None)
            .await
            .map_err(AnyError::from)
            .with_context(|| {
                if file_path.is_dir() {
                    // directory imports are not allowed when importing from an
                    // ES module, so provide the user with a helpful error message
                    let dir_path = file_path;
                    let mut msg = "Directory import ".to_string();
                    msg.push_str(&dir_path.to_string_lossy());
                    if let Some(referrer) = &maybe_referrer {
                        msg.push_str(" is not supported resolving import from ");
                        msg.push_str(referrer.as_str());
                        let entrypoint_name = ["index.mjs", "index.js", "index.cjs"]
                            .iter()
                            .find(|e| dir_path.join(e).is_file());
                        if let Some(entrypoint_name) = entrypoint_name {
                            msg.push_str("\nDid you mean to import ");
                            msg.push_str(entrypoint_name);
                            msg.push_str(" within the directory?");
                        }
                    }
                    msg
                } else {
                    let mut msg = "Unable to load ".to_string();
                    msg.push_str(&file_path.to_string_lossy());
                    if let Some(referrer) = &maybe_referrer {
                        msg.push_str(" imported from ");
                        msg.push_str(referrer.as_str());
                    }
                    msg
                }
            })?;

        let code = if self.cjs_resolutions.contains(specifier) {
            // translate cjs to esm if it's cjs and inject node globals
            let code = match String::from_utf8_lossy(&code) {
                Cow::Owned(code) => code,
                // SAFETY: `String::from_utf8_lossy` guarantees that the result is valid
                // UTF-8 if `Cow::Borrowed` is returned.
                Cow::Borrowed(_) => unsafe { String::from_utf8_unchecked(code) },
            };
            ModuleSourceCode::String(
                self.node_code_translator
                    .translate_cjs_to_esm(specifier, Some(code))
                    .await?
                    .into(),
            )
        } else {
            // esm and json code is untouched
            ModuleSourceCode::Bytes(code.into_boxed_slice().into())
        };
        Ok(ModuleCodeStringSource {
            code,
            found_url: specifier.clone(),
            media_type: MediaType::from_specifier(specifier),
        })
    }
}

/// Keeps track of what module specifiers were resolved as CJS.
#[derive(Debug, Default)]
pub struct CjsResolutionStore(DashSet<ModuleSpecifier>);

impl CjsResolutionStore {
    pub fn contains(&self, specifier: &ModuleSpecifier) -> bool {
        self.0.contains(specifier)
    }

    pub fn insert(&self, specifier: ModuleSpecifier) {
        self.0.insert(specifier);
    }
}

/// A resolver that takes care of resolution, taking into account loaded
/// import map, JSX settings.
#[derive(Debug)]
pub struct CliGraphResolver {
    node_resolver: Option<Arc<CliNodeResolver>>,
    npm_resolver: Option<Arc<dyn CliNpmResolver>>,
    workspace_resolver: Arc<WorkspaceResolver>,
    maybe_default_jsx_import_source: Option<String>,
    maybe_default_jsx_import_source_types: Option<String>,
    maybe_jsx_import_source_module: Option<String>,
    maybe_vendor_specifier: Option<ModuleSpecifier>,
    found_package_json_dep_flag: AtomicFlag,
    bare_node_builtins_enabled: bool,
}

pub struct CliGraphResolverOptions<'a> {
    pub node_resolver: Option<Arc<CliNodeResolver>>,
    pub npm_resolver: Option<Arc<dyn CliNpmResolver>>,
    pub workspace_resolver: Arc<WorkspaceResolver>,
    pub bare_node_builtins_enabled: bool,
    pub maybe_jsx_import_source_config: Option<JsxImportSourceConfig>,
    pub maybe_vendor_dir: Option<&'a PathBuf>,
}

impl CliGraphResolver {
    pub fn new(options: CliGraphResolverOptions) -> Self {
        Self {
            node_resolver: options.node_resolver,
            npm_resolver: options.npm_resolver,
            workspace_resolver: options.workspace_resolver,
            maybe_default_jsx_import_source: options
                .maybe_jsx_import_source_config
                .as_ref()
                .and_then(|c| c.default_specifier.clone()),
            maybe_default_jsx_import_source_types: options
                .maybe_jsx_import_source_config
                .as_ref()
                .and_then(|c| c.default_types_specifier.clone()),
            maybe_jsx_import_source_module: options
                .maybe_jsx_import_source_config
                .map(|c| c.module),
            maybe_vendor_specifier: options
                .maybe_vendor_dir
                .and_then(|v| ModuleSpecifier::from_directory_path(v).ok()),
            found_package_json_dep_flag: Default::default(),
            bare_node_builtins_enabled: options.bare_node_builtins_enabled,
        }
    }

    pub fn as_graph_resolver(&self) -> &dyn Resolver {
        self
    }

    pub fn create_graph_npm_resolver(&self) -> WorkerCliNpmGraphResolver {
        WorkerCliNpmGraphResolver {
            npm_resolver: self.npm_resolver.as_ref(),
            found_package_json_dep_flag: &self.found_package_json_dep_flag,
            bare_node_builtins_enabled: self.bare_node_builtins_enabled,
        }
    }

    // todo(dsherret): if we returned structured errors from the NodeResolver we wouldn't need this
    fn check_surface_byonm_node_error(
        &self,
        specifier: &str,
        referrer: &ModuleSpecifier,
        original_err: AnyError,
        resolver: &ByonmCliNpmResolver,
    ) -> Result<(), AnyError> {
        if let Ok((pkg_name, _, _)) = parse_npm_pkg_name(specifier, referrer) {
            match resolver.resolve_package_folder_from_package(&pkg_name, referrer) {
                Ok(_) => {
                    return Err(original_err);
                }
                Err(_) => {
                    if resolver
                        .find_ancestor_package_json_with_dep(&pkg_name, referrer)
                        .is_some()
                    {
                        return Err(anyhow!(
                            concat!(
                                "Could not resolve \"{}\", but found it in a package.json. ",
                                "Deno expects the node_modules/ directory to be up to date. ",
                                "Did you forget to run `npm install`?"
                            ),
                            specifier
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

impl Resolver for CliGraphResolver {
    fn default_jsx_import_source(&self) -> Option<String> {
        self.maybe_default_jsx_import_source.clone()
    }

    fn default_jsx_import_source_types(&self) -> Option<String> {
        self.maybe_default_jsx_import_source_types.clone()
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
        mode: ResolutionMode,
    ) -> Result<ModuleSpecifier, ResolveError> {
        fn to_node_mode(mode: ResolutionMode) -> NodeResolutionMode {
            match mode {
                ResolutionMode::Execution => NodeResolutionMode::Execution,
                ResolutionMode::Types => NodeResolutionMode::Types,
            }
        }

        let referrer = &referrer_range.specifier;
        let result: Result<_, ResolveError> = self
            .workspace_resolver
            .resolve(specifier, referrer)
            .map_err(|err| match err {
                MappedResolutionError::Specifier(err) => ResolveError::Specifier(err),
                MappedResolutionError::ImportMap(err) => ResolveError::Other(err.into()),
            });
        let result = match result {
            Ok(resolution) => match resolution {
                MappedResolution::Normal(specifier) | MappedResolution::ImportMap(specifier) => {
                    Ok(specifier)
                }
                // todo(dsherret): for byonm it should do resolution solely based on
                // the referrer and not the package.json
                MappedResolution::PackageJson {
                    dep_result,
                    alias,
                    sub_path,
                    ..
                } => {
                    // found a specifier in the package.json, so mark that
                    // we need to do an "npm install" later
                    self.found_package_json_dep_flag.raise();

                    dep_result
                        .as_ref()
                        .map_err(|e| ResolveError::Other(e.clone().into()))
                        .and_then(|dep| match dep {
                            PackageJsonDepValue::Req(req) => ModuleSpecifier::parse(&format!(
                                "npm:{}{}",
                                req,
                                sub_path.map(|s| format!("/{}", s)).unwrap_or_default()
                            ))
                            .map_err(|e| ResolveError::Other(e.into())),
                            PackageJsonDepValue::Workspace(version_req) => self
                                .workspace_resolver
                                .resolve_workspace_pkg_json_folder_for_pkg_json_dep(
                                    alias,
                                    version_req,
                                )
                                .map_err(|e| ResolveError::Other(e.into()))
                                .and_then(|pkg_folder| {
                                    Ok(self
                                        .node_resolver
                                        .as_ref()
                                        .unwrap()
                                        .resolve_package_sub_path_from_deno_module(
                                            pkg_folder,
                                            sub_path.as_deref(),
                                            Some(referrer),
                                            to_node_mode(mode),
                                        )?
                                        .into_url())
                                }),
                        })
                }
            },
            Err(err) => Err(err),
        };

        // check if it's an npm specifier that resolves to a workspace member
        if let Some(node_resolver) = &self.node_resolver {
            if let Ok(specifier) = &result {
                if let Ok(req_ref) = NpmPackageReqReference::from_specifier(specifier) {
                    if let Some(pkg_folder) = self
                        .workspace_resolver
                        .resolve_workspace_pkg_json_folder_for_npm_specifier(req_ref.req())
                    {
                        return Ok(node_resolver
                            .resolve_package_sub_path_from_deno_module(
                                pkg_folder,
                                req_ref.sub_path(),
                                Some(referrer),
                                to_node_mode(mode),
                            )?
                            .into_url());
                    }
                }
            }
        }

        // When the user is vendoring, don't allow them to import directly from the vendor/ directory
        // as it might cause them confusion or duplicate dependencies. Additionally, this folder has
        // special treatment in the language server so it will definitely cause issues/confusion there
        // if they do this.
        if let Some(vendor_specifier) = &self.maybe_vendor_specifier {
            if let Ok(specifier) = &result {
                if specifier.as_str().starts_with(vendor_specifier.as_str()) {
                    return Err(ResolveError::Other(anyhow!("Importing from the vendor directory is not permitted. Use a remote specifier instead or disable vendoring.")));
                }
            }
        }

        if let Some(resolver) = self.npm_resolver.as_ref().and_then(|r| r.as_byonm()) {
            match &result {
                Ok(specifier) => {
                    if let Ok(npm_req_ref) = NpmPackageReqReference::from_specifier(specifier) {
                        let node_resolver = self.node_resolver.as_ref().unwrap();
                        return node_resolver
                            .resolve_req_reference(&npm_req_ref, referrer, to_node_mode(mode))
                            .map(|res| res.into_url())
                            .map_err(|err| err.into());
                    }
                }
                Err(_) => {
                    if referrer.scheme() == "file" {
                        if let Some(node_resolver) = &self.node_resolver {
                            let node_result =
                                node_resolver.resolve(specifier, referrer, to_node_mode(mode));
                            match node_result {
                                Ok(Some(res)) => {
                                    return Ok(res.into_url());
                                }
                                Ok(None) => {
                                    self.check_surface_byonm_node_error(
                                        specifier,
                                        referrer,
                                        anyhow!("Cannot find \"{}\"", specifier),
                                        resolver,
                                    )
                                    .map_err(ResolveError::Other)?;
                                }
                                Err(err) => {
                                    self.check_surface_byonm_node_error(
                                        specifier, referrer, err, resolver,
                                    )
                                    .map_err(ResolveError::Other)?;
                                }
                            }
                        }
                    }
                }
            }
        }

        if referrer.scheme() == "file" {
            if let Some(node_resolver) = &self.node_resolver {
                let node_result = node_resolver.resolve_if_in_npm_package(
                    specifier,
                    referrer,
                    to_node_mode(mode),
                );
                if let Some(Ok(Some(res))) = node_result {
                    return Ok(res.into_url());
                }
            }
        }

        let specifier = result?;
        match &self.node_resolver {
            Some(node_resolver) => node_resolver
                .handle_if_in_node_modules(specifier)
                .map_err(|e| e.into()),
            None => Ok(specifier),
        }
    }
}

#[derive(Debug)]
pub struct WorkerCliNpmGraphResolver<'a> {
    npm_resolver: Option<&'a Arc<dyn CliNpmResolver>>,
    found_package_json_dep_flag: &'a AtomicFlag,
    bare_node_builtins_enabled: bool,
}

#[async_trait(?Send)]
impl<'a> deno_graph::source::NpmResolver for WorkerCliNpmGraphResolver<'a> {
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

    fn on_resolve_bare_builtin_node_module(&self, _module_name: &str, _range: &deno_graph::Range) {
        // let deno_graph::Range {
        //     start, specifier, ..
        // } = range;
        // let line = start.line + 1;
        // let column = start.character + 1;
        // if !*DENO_DISABLE_PEDANTIC_NODE_WARNINGS {
        //     log::warn!("Warning: Resolving \"{module_name}\" as \"node:{module_name}\" at {specifier}:{line}:{column}. If you want to use a built-in Node module, add a \"node:\" prefix.")
        // }
    }

    fn load_and_cache_npm_package_info(&self, package_name: &str) {
        match self.npm_resolver {
            Some(npm_resolver) if npm_resolver.as_managed().is_some() => {
                let npm_resolver = npm_resolver.clone();
                let package_name = package_name.to_string();
                deno_core::unsync::spawn(async move {
                    if let Some(managed) = npm_resolver.as_managed() {
                        let _ignore = managed.cache_package_info(&package_name).await;
                    }
                });
            }
            _ => {}
        }
    }

    async fn resolve_pkg_reqs(&self, package_reqs: &[PackageReq]) -> NpmResolvePkgReqsResult {
        match &self.npm_resolver {
            Some(npm_resolver) => {
                let npm_resolver = match npm_resolver.as_inner() {
                    InnerCliNpmResolverRef::Managed(npm_resolver) => npm_resolver,
                    // if we are using byonm, then this should never be called because
                    // we don't use deno_graph's npm resolution in this case
                    InnerCliNpmResolverRef::Byonm(_) => unreachable!(),
                };

                let top_level_result = if self.found_package_json_dep_flag.is_raised() {
                    npm_resolver
                        .ensure_top_level_package_json_install()
                        .await
                        .map(|_| ())
                } else {
                    Ok(())
                };

                let result = npm_resolver.add_package_reqs_raw(package_reqs).await;

                NpmResolvePkgReqsResult {
                    results: result
                        .results
                        .into_iter()
                        .map(|r| {
                            r.map_err(|err| match err {
                                NpmResolutionError::Registry(e) => {
                                    NpmLoadError::RegistryInfo(Arc::new(e.into()))
                                }
                                NpmResolutionError::Resolution(e) => {
                                    NpmLoadError::PackageReqResolution(Arc::new(e.into()))
                                }
                                NpmResolutionError::DependencyEntry(e) => {
                                    NpmLoadError::PackageReqResolution(Arc::new(e.into()))
                                }
                            })
                        })
                        .collect(),
                    dep_graph_result: match top_level_result {
                        Ok(()) => result.dependencies_result.map_err(Arc::new),
                        Err(err) => Err(Arc::new(err)),
                    },
                }
            }
            None => {
                let err = Arc::new(anyhow!(
                    "npm specifiers were requested; but --no-npm is specified"
                ));
                NpmResolvePkgReqsResult {
                    results: package_reqs
                        .iter()
                        .map(|_| Err(NpmLoadError::RegistryInfo(err.clone())))
                        .collect(),
                    dep_graph_result: Err(err),
                }
            }
        }
    }

    fn enables_bare_builtin_node_module(&self) -> bool {
        self.bare_node_builtins_enabled
    }
}
