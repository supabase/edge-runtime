use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use deno_ast::{MediaType, ModuleSpecifier};
use deno_core::futures;
use deno_core::futures::FutureExt;
use deno_graph::source::{CacheInfo, LoadFuture, LoadOptions, LoadResponse, Loader};

use npm::CliNpmResolver;
use sb_core::cache::cache_db::CacheDBHash;
use sb_core::cache::emit::EmitCache;
use sb_core::cache::fc_permissions::FcPermissions;
use sb_core::cache::module_info::ModuleInfoCache;
use sb_core::cache::{CacheSetting, GlobalHttpCache};
use sb_core::util::errors::get_error_class_name;

use crate::file_fetcher::{FetchNoFollowOptions, FetchOptions, FileFetcher, FileOrRedirect};

/// A "wrapper" for the FileFetcher and DiskCache for the Deno CLI that provides
/// a concise interface to the DENO_DIR when building module graphs.
#[allow(dead_code)]
pub struct FetchCacher {
    emit_cache: EmitCache,
    file_fetcher: Arc<FileFetcher>,
    file_header_overrides: HashMap<ModuleSpecifier, HashMap<String, String>>,
    global_http_cache: Arc<GlobalHttpCache>,
    npm_resolver: Arc<dyn CliNpmResolver>,
    module_info_cache: Arc<ModuleInfoCache>,
    permissions: FcPermissions,
    cache_info_enabled: bool,
}

impl FetchCacher {
    pub fn new(
        emit_cache: EmitCache,
        file_fetcher: Arc<FileFetcher>,
        file_header_overrides: HashMap<ModuleSpecifier, HashMap<String, String>>,
        global_http_cache: Arc<GlobalHttpCache>,
        npm_resolver: Arc<dyn CliNpmResolver>,
        module_info_cache: Arc<ModuleInfoCache>,
        permissions: FcPermissions,
    ) -> Self {
        Self {
            emit_cache,
            file_fetcher,
            file_header_overrides,
            global_http_cache,
            npm_resolver,
            module_info_cache,
            permissions,
            cache_info_enabled: false,
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

        if specifier.scheme() == "file" && specifier.path().contains("/node_modules/") {
            // The specifier might be in a completely different symlinked tree than
            // what the node_modules url is in (ex. `/my-project-1/node_modules`
            // symlinked to `/my-project-2/node_modules`), so first we checked if the path
            // is in a node_modules dir to avoid needlessly canonicalizing, then now compare
            // against the canonicalized specifier.
            let specifier = sb_core::node::resolve_specifier_into_node_modules(specifier);
            if self.npm_resolver.in_npm_package(&specifier) {
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
          let maybe_cache_setting = match options.cache_setting {
            LoaderCacheSetting::Use => None,
            LoaderCacheSetting::Reload => {
              if matches!(file_fetcher.cache_setting(), CacheSetting::Only) {
                return Err(deno_core::anyhow::anyhow!(
                  "Could not resolve version constraint using only cached data. Try running again without --cached-only"
                ));
              }
              Some(CacheSetting::ReloadAll)
            }
            LoaderCacheSetting::Only => Some(CacheSetting::Only),
          };
          file_fetcher
            .fetch_no_follow_with_options(FetchNoFollowOptions {
              fetch_options: FetchOptions {
                specifier: &specifier,
                permissions: permissions.clone(),
                maybe_accept: None,
                maybe_cache_setting: maybe_cache_setting.as_ref(),
              },
              maybe_checksum: options.maybe_checksum.as_ref(),
            })
            .await
            .map(|file_or_redirect| {
              match file_or_redirect {
                FileOrRedirect::File(file) => {
                  let maybe_headers =
                  match (file.maybe_headers, file_header_overrides.get(&specifier)) {
                    (Some(headers), Some(overrides)) => {
                      Some(headers.into_iter().chain(overrides.clone()).collect())
                    }
                    (Some(headers), None) => Some(headers),
                    (None, Some(overrides)) => Some(overrides.clone()),
                    (None, None) => None,
                  };
                Ok(Some(LoadResponse::Module {
                  specifier: file.specifier,
                  maybe_headers,
                  content: file.source,
                }))
                },
                FileOrRedirect::Redirect(redirect_specifier) => {
                  Ok(Some(LoadResponse::Redirect {
                    specifier: redirect_specifier,
                  }))
                },
              }
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
                "NotCached" if options.cache_setting == LoaderCacheSetting::Only => Ok(None),
                _ => Err(err),
              }
            })
        }
        .boxed_local()
    }

    fn cache_module_info(
        &self,
        specifier: &ModuleSpecifier,
        source: &Arc<[u8]>,
        module_info: &deno_graph::ModuleInfo,
    ) {
        let source_hash = CacheDBHash::from_source(source);
        let result = self.module_info_cache.set_module_info(
            specifier,
            MediaType::from_specifier(specifier),
            source_hash,
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
