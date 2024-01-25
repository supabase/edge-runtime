use crate::node::node_module_loader::CjsResolutionStore;
use anyhow::Context;
use deno_core::error::{generic_error, AnyError};
use deno_core::ModuleSpecifier;
use deno_semver::npm::NpmPackageReqReference;
use sb_node::{NodePermissions, NodeResolution, NodeResolutionMode, NodeResolver};
use sb_npm::CliNpmResolver;
use std::path::Path;
use std::sync::Arc;

pub struct CliNodeResolver {
    cjs_resolutions: Arc<CjsResolutionStore>,
    node_resolver: Arc<NodeResolver>,
    npm_resolver: Arc<dyn CliNpmResolver>,
}

impl CliNodeResolver {
    pub fn new(
        cjs_resolutions: Arc<CjsResolutionStore>,
        node_resolver: Arc<NodeResolver>,
        npm_resolver: Arc<dyn CliNpmResolver>,
    ) -> Self {
        Self {
            cjs_resolutions,
            node_resolver,
            npm_resolver,
        }
    }

    pub fn in_npm_package(&self, referrer: &ModuleSpecifier) -> bool {
        self.npm_resolver.in_npm_package(referrer)
    }

    pub fn resolve_if_in_npm_package(
        &self,
        specifier: &str,
        referrer: &ModuleSpecifier,
        permissions: &dyn NodePermissions,
    ) -> Option<Result<ModuleSpecifier, AnyError>> {
        if self.in_npm_package(referrer) {
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

    pub fn resolve_req_reference(
        &self,
        req_ref: &NpmPackageReqReference,
        permissions: &dyn NodePermissions,
        referrer: &ModuleSpecifier,
    ) -> Result<ModuleSpecifier, AnyError> {
        let package_folder = self
            .npm_resolver
            .resolve_pkg_folder_from_deno_module_req(req_ref.req(), referrer)?;
        self.resolve_package_sub_path(&package_folder, req_ref.sub_path(), referrer, permissions)
            .with_context(|| format!("Could not resolve '{}'.", req_ref))
    }

    pub fn resolve_package_sub_path(
        &self,
        package_folder: &Path,
        sub_path: Option<&str>,
        referrer: &ModuleSpecifier,
        permissions: &dyn NodePermissions,
    ) -> Result<ModuleSpecifier, AnyError> {
        self.handle_node_resolve_result(
            self.node_resolver.resolve_package_subpath_from_deno_module(
                package_folder,
                sub_path,
                referrer,
                NodeResolutionMode::Execution,
                permissions,
            ),
        )
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
