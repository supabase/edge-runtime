// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

pub mod byonm;
pub mod cache_dir;
pub mod common;
pub mod managed;

use deno_ast::ModuleSpecifier;
use deno_core::error::AnyError;
use deno_semver::package::PackageReq;
pub use managed::*;
use std::path::PathBuf;
use std::sync::Arc;

use crate::byonm::ByonmCliNpmResolver;
use sb_node::NpmResolver;
use serde::{Deserialize, Serialize};

/// State provided to the process via an environment variable.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NpmProcessState {
    pub kind: NpmProcessStateKind,
    pub local_node_modules_path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum NpmProcessStateKind {
    Snapshot(deno_npm::resolution::SerializedNpmResolutionSnapshot),
    Byonm,
}

pub enum InnerCliNpmResolverRef<'a> {
    Managed(&'a ManagedCliNpmResolver),
    #[allow(dead_code)]
    Byonm(&'a ByonmCliNpmResolver),
}

pub trait CliNpmResolver: NpmResolver {
    fn into_npm_resolver(self: Arc<Self>) -> Arc<dyn NpmResolver>;

    fn clone_snapshotted(&self) -> Arc<dyn CliNpmResolver>;

    fn as_inner(&self) -> InnerCliNpmResolverRef;

    fn as_managed(&self) -> Option<&ManagedCliNpmResolver> {
        match self.as_inner() {
            InnerCliNpmResolverRef::Managed(inner) => Some(inner),
            InnerCliNpmResolverRef::Byonm(_) => None,
        }
    }

    fn as_byonm(&self) -> Option<&ByonmCliNpmResolver> {
        match self.as_inner() {
            InnerCliNpmResolverRef::Managed(_) => None,
            InnerCliNpmResolverRef::Byonm(inner) => Some(inner),
        }
    }

    fn root_node_modules_path(&self) -> Option<&PathBuf>;

    fn resolve_pkg_folder_from_deno_module_req(
        &self,
        req: &PackageReq,
        referrer: &ModuleSpecifier,
    ) -> Result<PathBuf, AnyError>;

    /// Returns a hash returning the state of the npm resolver
    /// or `None` if the state currently can't be determined.
    fn check_state_hash(&self) -> Option<u64>;
}
