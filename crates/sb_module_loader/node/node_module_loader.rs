use crate::node::cjs_code_anaylzer::CliNodeCodeTranslator;
use crate::node::cli_node_resolver::CliNodeResolver;
use anyhow::Context;
use deno_ast::MediaType;
use deno_core::error::AnyError;
use deno_core::parking_lot::Mutex;
use deno_core::{ModuleCode, ModuleSpecifier};
use sb_node::NodePermissions;
use std::collections::HashSet;
use std::sync::Arc;

pub struct NpmModuleLoader {
    cjs_resolutions: Arc<CjsResolutionStore>,
    node_code_translator: Arc<CliNodeCodeTranslator>,
    fs: Arc<dyn deno_fs::FileSystem>,
    node_resolver: Arc<CliNodeResolver>,
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
        node_resolver: Arc<CliNodeResolver>,
    ) -> Self {
        Self {
            cjs_resolutions,
            node_code_translator,
            fs,
            node_resolver,
        }
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
