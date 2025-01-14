// use std::borrow::Cow;
// use std::path::Path;
// use std::path::PathBuf;
// use std::sync::Arc;

// use anyhow::anyhow;
// use anyhow::Context;
// use async_trait::async_trait;
// use dashmap::DashSet;
// use deno::node::CliNodeCodeTranslator;
// use deno::node_resolver::errors::ClosestPkgJsonError;
// use deno::node_resolver::parse_npm_pkg_name;
// use deno::node_resolver::NodeResolution;
// use deno::npm::byonm::CliByonmNpmResolver;
// use deno::npm::CliNpmResolver;
// use deno::npm::InnerCliNpmResolverRef;
// use deno_ast::MediaType;
// use deno_config::deno_json::JsxImportSourceConfig;
// use deno_config::workspace::MappedResolution;
// use deno_config::workspace::MappedResolutionError;
// use deno_config::workspace::WorkspaceResolver;
// use deno_core::error::AnyError;
// use deno_core::unsync::sync::AtomicFlag;
// use deno_core::ModuleSourceCode;
// use deno_core::ModuleSpecifier;
// use deno_graph::source::ResolutionKind;
// use deno_graph::source::ResolutionMode;
// use deno_graph::source::ResolveError;
// use deno_graph::source::Resolver;
// use deno_graph::source::UnknownBuiltInNodeModuleError;
// use deno_graph::source::DEFAULT_JSX_IMPORT_SOURCE_MODULE;
// use deno_graph::NpmLoadError;
// use deno_graph::NpmResolvePkgReqsResult;
// use deno_npm::resolution::NpmResolutionError;
// use deno_package_json::NodeModuleKind;
// use deno_package_json::PackageJsonDepValue;
// use deno_semver::npm::NpmPackageReqReference;
// use deno_semver::package::PackageReq;
// use ext_node::is_builtin_node_module;
// use ext_node::NodeResolver;
// use ext_node::PackageJson;

// pub struct ModuleCodeStringSource {
//   pub code: ModuleSourceCode,
//   pub found_url: ModuleSpecifier,
//   pub media_type: MediaType,
// }

// #[derive(Debug)]
// pub struct CliNodeResolver {
//   cjs_resolutions: Arc<CjsResolutionStore>,
//   fs: Arc<dyn deno_fs::FileSystem>,
//   node_resolver: Arc<NodeResolver>,
//   // todo(dsherret): remove this pub(crate)
//   pub(crate) npm_resolver: Arc<dyn CliNpmResolver>,
// }

// impl CliNodeResolver {
//   pub fn new(
//     cjs_resolutions: Arc<CjsResolutionStore>,
//     fs: Arc<dyn deno_fs::FileSystem>,
//     node_resolver: Arc<NodeResolver>,
//     npm_resolver: Arc<dyn CliNpmResolver>,
//   ) -> Self {
//     Self {
//       cjs_resolutions,
//       fs,
//       node_resolver,
//       npm_resolver,
//     }
//   }

//   pub fn in_npm_package(&self, specifier: &ModuleSpecifier) -> bool {
//     self.npm_resolver.in_npm_package(specifier)
//   }

//   pub fn get_closest_package_json(
//     &self,
//     referrer: &ModuleSpecifier,
//   ) -> Result<Option<Arc<PackageJson>>, ClosestPkgJsonError> {
//     self.node_resolver.get_closest_package_json(referrer)
//   }

//   pub fn resolve_if_in_npm_package(
//     &self,
//     specifier: &str,
//     referrer: &ModuleSpecifier,
//     mode: NodeResolutionMode,
//   ) -> Option<Result<Option<NodeResolution>, AnyError>> {
//     if self.in_npm_package(referrer) {
//       // we're in an npm package, so use node resolution
//       Some(self.resolve(specifier, referrer, mode))
//     } else {
//       None
//     }
//   }

//   pub fn resolve(
//     &self,
//     specifier: &str,
//     referrer: &ModuleSpecifier,
//     mode: NodeResolutionMode,
//   ) -> Result<Option<NodeResolution>, AnyError> {
//     let referrer_kind = if self.cjs_resolutions.contains(referrer) {
//       NodeModuleKind::Cjs
//     } else {
//       NodeModuleKind::Esm
//     };

//     self.handle_node_resolve_result(
//       self
//         .node_resolver
//         .resolve(specifier, referrer, referrer_kind, mode)
//         .map_err(AnyError::from),
//     )
//   }

//   pub fn resolve_req_reference(
//     &self,
//     req_ref: &NpmPackageReqReference,
//     referrer: &ModuleSpecifier,
//     mode: NodeResolutionMode,
//   ) -> Result<NodeResolution, AnyError> {
//     self.resolve_req_with_sub_path(
//       req_ref.req(),
//       req_ref.sub_path(),
//       referrer,
//       mode,
//     )
//   }

//   pub fn resolve_req_with_sub_path(
//     &self,
//     req: &PackageReq,
//     sub_path: Option<&str>,
//     referrer: &ModuleSpecifier,
//     mode: NodeResolutionMode,
//   ) -> Result<NodeResolution, AnyError> {
//     let package_folder = self
//       .npm_resolver
//       .resolve_pkg_folder_from_deno_module_req(req, referrer)?;
//     let maybe_resolution = self
//       .maybe_resolve_package_sub_path_from_deno_module(
//         &package_folder,
//         sub_path,
//         Some(referrer),
//         mode,
//       )?;
//     match maybe_resolution {
//       Some(resolution) => Ok(resolution),
//       None => {
//         if self.npm_resolver.as_byonm().is_some() {
//           let package_json_path = package_folder.join("package.json");
//           if !self.fs.exists_sync(&package_json_path) {
//             return Err(anyhow!(
//                             "Could not find '{}'. Deno expects the node_modules/ directory to be up to date. Did you forget to run `npm install`?",
//                             package_json_path.display()
//                             ));
//           }
//         }
//         Err(anyhow!(
//           "Failed resolving '{}{}' in '{}'.",
//           req,
//           sub_path.map(|s| format!("/{}", s)).unwrap_or_default(),
//           package_folder.display()
//         ))
//       }
//     }
//   }

//   pub fn resolve_package_sub_path_from_deno_module(
//     &self,
//     package_folder: &Path,
//     sub_path: Option<&str>,
//     maybe_referrer: Option<&ModuleSpecifier>,
//     mode: NodeResolutionMode,
//   ) -> Result<NodeResolution, AnyError> {
//     self
//       .maybe_resolve_package_sub_path_from_deno_module(
//         package_folder,
//         sub_path,
//         maybe_referrer,
//         mode,
//       )?
//       .ok_or_else(|| {
//         anyhow!(
//           "Failed resolving '{}' in '{}'.",
//           sub_path
//             .map(|s| format!("/{}", s))
//             .unwrap_or_else(|| ".".to_string()),
//           package_folder.display(),
//         )
//       })
//   }

//   pub fn maybe_resolve_package_sub_path_from_deno_module(
//     &self,
//     package_folder: &Path,
//     sub_path: Option<&str>,
//     maybe_referrer: Option<&ModuleSpecifier>,
//     mode: NodeResolutionMode,
//   ) -> Result<Option<NodeResolution>, AnyError> {
//     self.handle_node_resolve_result(
//       self
//         .node_resolver
//         .resolve_package_subpath_from_deno_module(
//           package_folder,
//           sub_path,
//           maybe_referrer,
//           mode,
//         )
//         .map_err(AnyError::from),
//     )
//   }

//   pub fn handle_if_in_node_modules(
//     &self,
//     specifier: ModuleSpecifier,
//   ) -> Result<ModuleSpecifier, AnyError> {
//     // skip canonicalizing if we definitely know it's unnecessary
//     if specifier.scheme() == "file"
//       && specifier.path().contains("/node_modules/")
//     {
//       // Specifiers in the node_modules directory are canonicalized
//       // so canoncalize then check if it's in the node_modules directory.
//       // If so, check if we need to store this specifier as being a CJS
//       // resolution.
//       let specifier =
//         ext_core::node::resolve_specifier_into_node_modules(&specifier);
//       if self.in_npm_package(&specifier) {
//         let resolution =
//           self.node_resolver.url_to_node_resolution(specifier)?;
//         if let NodeResolution::CommonJs(specifier) = &resolution {
//           self.cjs_resolutions.insert(specifier.clone());
//         }
//         return Ok(resolution.into_url());
//       }
//     }

//     Ok(specifier)
//   }

//   pub fn url_to_node_resolution(
//     &self,
//     specifier: ModuleSpecifier,
//   ) -> Result<NodeResolution, UrlToNodeResolutionError> {
//     self.node_resolver.url_to_node_resolution(specifier)
//   }

//   fn handle_node_resolve_result(
//     &self,
//     result: Result<Option<NodeResolution>, AnyError>,
//   ) -> Result<Option<NodeResolution>, AnyError> {
//     match result? {
//       Some(response) => {
//         if let NodeResolution::CommonJs(specifier) = &response {
//           // remember that this was a common js resolution
//           self.cjs_resolutions.insert(specifier.clone());
//         }
//         Ok(Some(response))
//       }
//       None => Ok(None),
//     }
//   }
// }

// #[derive(Clone)]
// pub struct NpmModuleLoader {
//   cjs_resolutions: Arc<CjsResolutionStore>,
//   node_code_translator: Arc<CliNodeCodeTranslator>,
//   fs: Arc<dyn deno_fs::FileSystem>,
//   node_resolver: Arc<CliNodeResolver>,
// }

// impl NpmModuleLoader {
//   pub fn new(
//     cjs_resolutions: Arc<CjsResolutionStore>,
//     node_code_translator: Arc<CliNodeCodeTranslator>,
//     fs: Arc<dyn deno_fs::FileSystem>,
//     node_resolver: Arc<CliNodeResolver>,
//   ) -> Self {
//     Self {
//       cjs_resolutions,
//       node_code_translator,
//       fs,
//       node_resolver,
//     }
//   }

//   pub async fn load_if_in_npm_package(
//     &self,
//     specifier: &ModuleSpecifier,
//     maybe_referrer: Option<&ModuleSpecifier>,
//   ) -> Option<Result<ModuleCodeStringSource, AnyError>> {
//     if self.node_resolver.in_npm_package(specifier) {
//       Some(self.load(specifier, maybe_referrer).await)
//     } else {
//       None
//     }
//   }

//   pub async fn load(
//     &self,
//     specifier: &ModuleSpecifier,
//     maybe_referrer: Option<&ModuleSpecifier>,
//   ) -> Result<ModuleCodeStringSource, AnyError> {
//     let file_path = specifier.to_file_path().unwrap();
//     let code = self
//       .fs
//       .read_file_async(file_path.clone(), None)
//       .await
//       .map_err(AnyError::from)
//       .with_context(|| {
//         if file_path.is_dir() {
//           // directory imports are not allowed when importing from an
//           // ES module, so provide the user with a helpful error message
//           let dir_path = file_path;
//           let mut msg = "Directory import ".to_string();
//           msg.push_str(&dir_path.to_string_lossy());
//           if let Some(referrer) = &maybe_referrer {
//             msg.push_str(" is not supported resolving import from ");
//             msg.push_str(referrer.as_str());
//             let entrypoint_name = ["index.mjs", "index.js", "index.cjs"]
//               .iter()
//               .find(|e| dir_path.join(e).is_file());
//             if let Some(entrypoint_name) = entrypoint_name {
//               msg.push_str("\nDid you mean to import ");
//               msg.push_str(entrypoint_name);
//               msg.push_str(" within the directory?");
//             }
//           }
//           msg
//         } else {
//           let mut msg = "Unable to load ".to_string();
//           msg.push_str(&file_path.to_string_lossy());
//           if let Some(referrer) = &maybe_referrer {
//             msg.push_str(" imported from ");
//             msg.push_str(referrer.as_str());
//           }
//           msg
//         }
//       })?;

//     let code = if self.cjs_resolutions.contains(specifier) {
//       // translate cjs to esm if it's cjs and inject node globals
//       let code = match String::from_utf8_lossy(&code) {
//         Cow::Owned(code) => code,
//         // SAFETY: `String::from_utf8_lossy` guarantees that the result is valid
//         // UTF-8 if `Cow::Borrowed` is returned.
//         Cow::Borrowed(_) => unsafe { String::from_utf8_unchecked(code) },
//       };
//       ModuleSourceCode::String(
//         self
//           .node_code_translator
//           .translate_cjs_to_esm(specifier, Some(code))
//           .await?
//           .into(),
//       )
//     } else {
//       // esm and json code is untouched
//       ModuleSourceCode::Bytes(code.into_boxed_slice().into())
//     };
//     Ok(ModuleCodeStringSource {
//       code,
//       found_url: specifier.clone(),
//       media_type: MediaType::from_specifier(specifier),
//     })
//   }
// }

// /// Keeps track of what module specifiers were resolved as CJS.
// #[derive(Debug, Default)]
// pub struct CjsResolutionStore(DashSet<ModuleSpecifier>);

// impl CjsResolutionStore {
//   pub fn contains(&self, specifier: &ModuleSpecifier) -> bool {
//     self.0.contains(specifier)
//   }

//   pub fn insert(&self, specifier: ModuleSpecifier) {
//     self.0.insert(specifier);
//   }
// }

// /// A resolver that takes care of resolution, taking into account loaded
// /// import map, JSX settings.
// #[derive(Debug)]
// pub struct CliGraphResolver {
//   node_resolver: Option<Arc<CliNodeResolver>>,
//   npm_resolver: Option<Arc<dyn CliNpmResolver>>,
//   workspace_resolver: Arc<WorkspaceResolver>,
//   maybe_default_jsx_import_source: Option<String>,
//   maybe_default_jsx_import_source_types: Option<String>,
//   maybe_jsx_import_source_module: Option<String>,
//   maybe_vendor_specifier: Option<ModuleSpecifier>,
//   found_package_json_dep_flag: AtomicFlag,
//   bare_node_builtins_enabled: bool,
// }

// pub struct CliGraphResolverOptions<'a> {
//   pub node_resolver: Option<Arc<CliNodeResolver>>,
//   pub npm_resolver: Option<Arc<dyn CliNpmResolver>>,
//   pub workspace_resolver: Arc<WorkspaceResolver>,
//   pub bare_node_builtins_enabled: bool,
//   pub maybe_jsx_import_source_config: Option<JsxImportSourceConfig>,
//   pub maybe_vendor_dir: Option<&'a PathBuf>,
// }

// impl CliGraphResolver {
//   pub fn new(options: CliGraphResolverOptions) -> Self {
//     Self {
//       node_resolver: options.node_resolver,
//       npm_resolver: options.npm_resolver,
//       workspace_resolver: options.workspace_resolver,
//       maybe_default_jsx_import_source: options
//         .maybe_jsx_import_source_config
//         .as_ref()
//         .and_then(|c| c.default_specifier.clone()),
//       maybe_default_jsx_import_source_types: options
//         .maybe_jsx_import_source_config
//         .as_ref()
//         .and_then(|c| c.default_types_specifier.clone()),
//       maybe_jsx_import_source_module: options
//         .maybe_jsx_import_source_config
//         .map(|c| c.module),
//       maybe_vendor_specifier: options
//         .maybe_vendor_dir
//         .and_then(|v| ModuleSpecifier::from_directory_path(v).ok()),
//       found_package_json_dep_flag: Default::default(),
//       bare_node_builtins_enabled: options.bare_node_builtins_enabled,
//     }
//   }

//   pub fn as_graph_resolver(&self) -> &dyn Resolver {
//     self
//   }
// }

// impl deno_graph::source::Resolver for CliGraphResolver {
//   fn default_jsx_import_source(&self) -> Option<String> {
//     self.maybe_default_jsx_import_source.clone()
//   }

//   fn default_jsx_import_source_types(&self) -> Option<String> {
//     self.maybe_default_jsx_import_source_types.clone()
//   }

//   fn jsx_import_source_module(&self) -> &str {
//     self
//       .maybe_jsx_import_source_module
//       .as_deref()
//       .unwrap_or(DEFAULT_JSX_IMPORT_SOURCE_MODULE)
//   }

//   fn resolve(
//     &self,
//     raw_specifier: &str,
//     referrer_range: &deno_graph::Range,
//     resolution_kind: ResolutionKind,
//   ) -> Result<ModuleSpecifier, ResolveError> {
//     self.resolver.resolve(
//       raw_specifier,
//       &referrer_range.specifier,
//       referrer_range.range.start,
//       referrer_range
//         .resolution_mode
//         .map(to_node_resolution_mode)
//         .unwrap_or_else(|| {
//           self
//             .cjs_tracker
//             .get_referrer_kind(&referrer_range.specifier)
//         }),
//       to_node_resolution_kind(resolution_kind),
//     )
//   }
// }

// pub fn to_node_resolution_kind(
//   kind: ResolutionKind,
// ) -> node_resolver::NodeResolutionKind {
//   match kind {
//     ResolutionKind::Execution => node_resolver::NodeResolutionKind::Execution,
//     ResolutionKind::Types => node_resolver::NodeResolutionKind::Types,
//   }
// }

// pub fn to_node_resolution_mode(
//   mode: deno_graph::source::ResolutionMode,
// ) -> node_resolver::ResolutionMode {
//   match mode {
//     deno_graph::source::ResolutionMode::Import => {
//       node_resolver::ResolutionMode::Import
//     }
//     deno_graph::source::ResolutionMode::Require => {
//       node_resolver::ResolutionMode::Require
//     }
//   }
// }
