use crate::node::cjs_code_anaylzer::CliNodeCodeTranslator;
use anyhow::Context;
use deno_ast::MediaType;
use deno_core::error::{generic_error, AnyError};
use deno_core::parking_lot::Mutex;
use deno_core::{ModuleCode, ModuleSpecifier};
use deno_semver::npm::{NpmPackageNvReference, NpmPackageReqReference};
use sb_node::{NodePermissions, NodeResolution, NodeResolutionMode, NodeResolver};
use std::collections::HashSet;
use std::sync::Arc;

pub struct NpmModuleLoader {
    cjs_resolutions: Arc<CjsResolutionStore>,
    node_code_translator: Arc<CliNodeCodeTranslator>,
    fs: Arc<dyn deno_fs::FileSystem>,
    node_resolver: Arc<NodeResolver>,
}

pub struct ModuleCodeSource {
    pub code: ModuleCode,
    pub found_url: ModuleSpecifier,
    pub media_type: MediaType,
}

impl NpmModuleLoader {
    pub fn new(
        cjs_resolutions: Arc<CjsResolutionStore>,
        node_code_translator: Arc<CliNodeCodeTranslator>,
        fs: Arc<dyn deno_fs::FileSystem>,
        node_resolver: Arc<NodeResolver>,
    ) -> Self {
        Self {
            cjs_resolutions,
            node_code_translator,
            fs,
            node_resolver,
        }
    }

    pub fn resolve_if_in_npm_package(
        &self,
        specifier: &str,
        referrer: &ModuleSpecifier,
        permissions: &dyn NodePermissions,
    ) -> Option<Result<ModuleSpecifier, AnyError>> {
        if self.node_resolver.in_npm_package(referrer) {
            // we're in an npm package, so use node resolution
            Some(
                self.handle_node_resolve_result(self.node_resolver.resolve(
                    specifier,
                    referrer,
                    NodeResolutionMode::Execution,
                    permissions,
                ))
                .with_context(|| format!("Could not resolve '{specifier}' from '{referrer}'.")),
            )
        } else {
            None
        }
    }

    pub fn resolve_nv_ref(
        &self,
        nv_ref: &NpmPackageNvReference,
        permissions: &dyn NodePermissions,
    ) -> Result<ModuleSpecifier, AnyError> {
        self.handle_node_resolve_result(self.node_resolver.resolve_npm_reference(
            nv_ref,
            NodeResolutionMode::Execution,
            permissions,
        ))
        .with_context(|| format!("Could not resolve '{}'.", nv_ref))
    }

    pub fn resolve_req_reference(
        &self,
        reference: &NpmPackageReqReference,
        permissions: &dyn NodePermissions,
    ) -> Result<ModuleSpecifier, AnyError> {
        self.handle_node_resolve_result(self.node_resolver.resolve_npm_req_reference(
            reference,
            NodeResolutionMode::Execution,
            permissions,
        ))
        .with_context(|| format!("Could not resolve '{reference}'."))
    }

    pub fn maybe_prepare_load(&self, specifier: &ModuleSpecifier) -> Option<Result<(), AnyError>> {
        if self.node_resolver.in_npm_package(specifier) {
            // nothing to prepare
            Some(Ok(()))
        } else {
            None
        }
    }

    pub fn load_sync_if_in_npm_package(
        &self,
        specifier: &ModuleSpecifier,
        maybe_referrer: Option<&ModuleSpecifier>,
        permissions: &dyn NodePermissions,
    ) -> Option<Result<ModuleCodeSource, AnyError>> {
        if self.node_resolver.in_npm_package(specifier) {
            Some(self.load_sync(specifier, maybe_referrer, permissions))
        } else {
            None
        }
    }

    fn load_sync(
        &self,
        specifier: &ModuleSpecifier,
        maybe_referrer: Option<&ModuleSpecifier>,
        permissions: &dyn NodePermissions,
    ) -> Result<ModuleCodeSource, AnyError> {
        let file_path = specifier.to_file_path().unwrap();
        let code = self
            .fs
            .read_text_file_sync(&file_path)
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
            self.node_code_translator.translate_cjs_to_esm(
                specifier,
                Some(code.as_str()),
                permissions,
            )?
        } else {
            // esm and json code is untouched
            code
        };
        Ok(ModuleCodeSource {
            code: code.into(),
            found_url: specifier.clone(),
            media_type: MediaType::from_specifier(specifier),
        })
    }

    fn handle_node_resolve_result(
        &self,
        result: Result<Option<NodeResolution>, AnyError>,
    ) -> Result<ModuleSpecifier, AnyError> {
        let response = match result? {
            Some(response) => response,
            None => return Err(generic_error("not found")),
        };
        if let NodeResolution::CommonJs(specifier) = &response {
            // remember that this was a common js resolution
            self.cjs_resolutions.insert(specifier.clone());
        }
        Ok(response.into_url())
    }
}

#[derive(Default)]
pub struct CjsResolutionStore(Mutex<HashSet<ModuleSpecifier>>);
impl CjsResolutionStore {
    pub fn contains(&self, specifier: &ModuleSpecifier) -> bool {
        self.0.lock().contains(specifier)
    }

    pub fn insert(&self, specifier: ModuleSpecifier) {
        self.0.lock().insert(specifier);
    }
}
