use crate::cache::emit::EmitCache;
use crate::cache::fc_permissions::FcPermissions;
use crate::cache::module_info::{ModuleInfoCache, ModuleInfoCacheSourceHash};
use crate::cache::parsed_source::ParsedSourceCache;
use crate::cache::{CacheSetting, GlobalHttpCache};
use crate::file_fetcher::{FetchOptions, FileFetcher};
use crate::util::errors::get_error_class_name;
use crate::util::fs::canonicalize_path_maybe_not_exists;
use deno_ast::{MediaType, ModuleSpecifier};
use deno_core::futures;
use deno_core::futures::FutureExt;
use deno_graph::source::{CacheInfo, LoadFuture, LoadOptions, LoadResponse, Loader};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub fn resolve_specifier_into_node_modules(specifier: &ModuleSpecifier) -> ModuleSpecifier {
    specifier
        .to_file_path()
        .ok()
        // this path might not exist at the time the graph is being created
        // because the node_modules folder might not yet exist
        .and_then(|path| canonicalize_path_maybe_not_exists(&path).ok())
        .and_then(|path| ModuleSpecifier::from_file_path(path).ok())
        .unwrap_or_else(|| specifier.clone())
}

/// A "wrapper" for the FileFetcher and DiskCache for the Deno CLI that provides
/// a concise interface to the DENO_DIR when building module graphs.
#[allow(dead_code)]
pub struct FetchCacher {
    module_info_cache: Arc<ModuleInfoCache>,
    emit_cache: EmitCache,
    file_fetcher: Arc<FileFetcher>,
    file_header_overrides: HashMap<ModuleSpecifier, HashMap<String, String>>,
    global_http_cache: Arc<GlobalHttpCache>,
    parsed_source_cache: Arc<ParsedSourceCache>,
    permissions: FcPermissions,
    cache_info_enabled: bool,
    maybe_local_node_modules_url: Option<ModuleSpecifier>,
}

impl FetchCacher {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        module_info_cache: Arc<ModuleInfoCache>,
        emit_cache: EmitCache,
        file_fetcher: Arc<FileFetcher>,
        file_header_overrides: HashMap<ModuleSpecifier, HashMap<String, String>>,
        global_http_cache: Arc<GlobalHttpCache>,
        parsed_source_cache: Arc<ParsedSourceCache>,
        permissions: FcPermissions,
        maybe_local_node_modules_url: Option<ModuleSpecifier>,
    ) -> Self {
        Self {
            module_info_cache,
            emit_cache,
            file_fetcher,
            file_header_overrides,
            global_http_cache,
            parsed_source_cache,
            permissions,
            cache_info_enabled: false,
            maybe_local_node_modules_url,
        }
    }

    /// The cache information takes a bit of time to fetch and it's
    /// not always necessary. It should only be enabled for deno info.
    pub fn enable_loading_cache_info(&mut self) {
        self.cache_info_enabled = true;
    }

    // DEPRECATED: Where the file is stored and how it's stored should be an implementation
    // detail of the cache.
    //
    // todo(dsheret): remove once implementing
    //  * https://github.com/denoland/deno/issues/17707
    //  * https://github.com/denoland/deno/issues/17703
    #[deprecated(
        note = "There should not be a way to do this because the file may not be cached at a local path in the future."
    )]
    fn get_local_path(&self, specifier: &ModuleSpecifier) -> Option<PathBuf> {
        // TODO(@kitsonk) fix when deno_graph does not query cache for synthetic
        // modules
        if specifier.scheme() == "flags" {
            None
        } else if specifier.scheme() == "file" {
            specifier.to_file_path().ok()
        } else {
            #[allow(deprecated)]
            self.global_http_cache
                .get_global_cache_filepath(specifier)
                .ok()
        }
    }
}

impl Loader for FetchCacher {
    fn get_cache_info(&self, specifier: &ModuleSpecifier) -> Option<CacheInfo> {
        if !self.cache_info_enabled {
            return None;
        }

        #[allow(deprecated)]
        let local = self.get_local_path(specifier)?;
        if local.is_file() {
            let emit = self
                .emit_cache
                .get_emit_filepath(specifier)
                .filter(|p| p.is_file());
            Some(CacheInfo {
                local: Some(local),
                emit,
                map: None,
            })
        } else {
            None
        }
    }

    fn load(&self, specifier: &ModuleSpecifier, options: LoadOptions) -> LoadFuture {
        use deno_graph::source::CacheSetting as LoaderCacheSetting;
        let cache_setting = options.cache_setting;

        let path = specifier.path();

        if path.contains("/node_modules/") {
            // The specifier might be in a completely different symlinked tree than
            // what the node_modules url is in (ex. `/my-project-1/node_modules`
            // symlinked to `/my-project-2/node_modules`), so first we checked if the path
            // is in a node_modules dir to avoid needlessly canonicalizing, then now compare
            // against the canonicalized specifier.
            let specifier = resolve_specifier_into_node_modules(specifier);
            if specifier.as_str().starts_with(path) {
                return Box::pin(futures::future::ready(Ok(Some(LoadResponse::External {
                    specifier,
                }))));
            }
        }

        let file_fetcher = self.file_fetcher.clone();
        let file_header_overrides = self.file_header_overrides.clone();
        let permissions = self.permissions.clone();
        let specifier = specifier.clone();

        async move {
            let maybe_cache_setting = match cache_setting {
                LoaderCacheSetting::Use => None,
                LoaderCacheSetting::Reload => {
                    if matches!(file_fetcher.cache_setting(), CacheSetting::Only) {
                        return Err(deno_core::anyhow::anyhow!(
                            "Failed to resolve version constraint. Try running again without --cached-only"
                        ));
                    }
                    Some(CacheSetting::ReloadAll)
                }
                LoaderCacheSetting::Only => Some(CacheSetting::Only),
            };
            file_fetcher
                .fetch_with_options(FetchOptions {
                    specifier: &specifier,
                    permissions,
                    maybe_accept: None,
                    maybe_cache_setting: maybe_cache_setting.as_ref(),
                })
                .await
                .map(|file| {
                    let maybe_headers = match (file.maybe_headers, file_header_overrides.get(&specifier)) {
                        (Some(headers), Some(overrides)) => Some(headers.into_iter().chain(overrides.clone()).collect()),
                        (Some(headers), None) => Some(headers),
                        (None, Some(overrides)) => Some(overrides.clone()),
                        (None, None) => None,
                    };
                    Ok(Some(LoadResponse::Module {
                        specifier: file.specifier,
                        maybe_headers,
                        content: file.source.into(),
                    }))
                })
                .unwrap_or_else(|err| {
                    if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
                        if io_err.kind() == std::io::ErrorKind::NotFound {
                            return Ok(None);
                        } else {
                            return Err(err);
                        }
                    }
                    let error_class_name = get_error_class_name(&err);
                    match error_class_name {
                        "NotFound" => Ok(None),
                        "NotCached" if cache_setting == LoaderCacheSetting::Only => Ok(None),
                        _ => Err(err),
                    }
                })
        }
        .boxed()
    }

    fn cache_module_info(
        &self,
        specifier: &ModuleSpecifier,
        source: &Arc<[u8]>,
        module_info: &deno_graph::ModuleInfo,
    ) {
        let source_hash = ModuleInfoCacheSourceHash::from_source(source);
        let result = self.module_info_cache.set_module_info(
            specifier,
            MediaType::from_specifier(specifier),
            &source_hash,
            module_info,
        );
        if let Err(err) = result {
            log::debug!(
                "Error saving module cache info for {}. {:#}",
                specifier,
                err
            );
        }
    }
}
