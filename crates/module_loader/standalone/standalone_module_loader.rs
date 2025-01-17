// Copyright 2018-2024 the Deno authors. All rights reserved. MIT license.

use base64::Engine;
use deno::deno_ast::MediaType;
use deno::deno_package_json::PackageJsonDepValue;
use deno::deno_semver::npm::NpmPackageReqReference;
use deno::node_resolver::NodeResolutionKind;
use deno::node_resolver::ResolutionMode;
use deno::resolver::CjsTracker;
use deno::resolver::CliNpmReqResolver;
use deno_config::workspace::MappedResolution;
use deno_config::workspace::WorkspaceResolver;
use deno_core::error::generic_error;
use deno_core::error::type_error;
use deno_core::error::AnyError;
use deno_core::futures::FutureExt;
use deno_core::url::Url;
use deno_core::ModuleLoader;
use deno_core::ModuleSourceCode;
use deno_core::ModuleSpecifier;
use deno_core::ModuleType;
use deno_core::RequestedModuleType;
use deno_core::ResolutionKind;
use eszip::deno_graph;
use eszip::EszipRelativeFileBaseUrl;
use eszip::ModuleKind;
use eszip_async_trait::AsyncEszipDataRead;
// use ext_node::NodeResolutionMode;
// use graph::resolver::CliNodeResolver;
use deno::resolver::NpmModuleLoader;
use deno_facade::LazyLoadableEszip;
use ext_node::NodeResolver;
use std::sync::Arc;
use tracing::instrument;

use crate::util::arc_u8_to_arc_str;

pub struct WorkspaceEszipModule {
  specifier: ModuleSpecifier,
  inner: eszip::Module,
}

pub struct WorkspaceEszip {
  pub eszip: LazyLoadableEszip,
  pub root_dir_url: Arc<Url>,
}

impl WorkspaceEszip {
  pub fn get_module(
    &self,
    specifier: &ModuleSpecifier,
  ) -> Option<WorkspaceEszipModule> {
    if specifier.scheme() == "file" {
      let specifier_key = EszipRelativeFileBaseUrl::new(&self.root_dir_url)
        .specifier_key(specifier);

      let module = self.eszip.ensure_module(&specifier_key)?;
      let specifier = self.root_dir_url.join(&module.specifier).unwrap();

      Some(WorkspaceEszipModule {
        specifier,
        inner: module,
      })
    } else {
      let module = self.eszip.ensure_module(specifier.as_str())?;

      Some(WorkspaceEszipModule {
        specifier: ModuleSpecifier::parse(&module.specifier).unwrap(),
        inner: module,
      })
    }
  }
}

pub struct SharedModuleLoaderState {
  pub(crate) eszip: WorkspaceEszip,
  pub(crate) workspace_resolver: WorkspaceResolver,
  pub(crate) cjs_tracker: Arc<CjsTracker>,
  pub(crate) npm_module_loader: Arc<NpmModuleLoader>,
  pub(crate) npm_req_resolver: Arc<CliNpmReqResolver>,
  pub(crate) node_resolver: Arc<NodeResolver>,
}

#[derive(Clone)]
pub struct EmbeddedModuleLoader {
  pub(crate) shared: Arc<SharedModuleLoaderState>,
  pub(crate) include_source_map: bool,
}

impl ModuleLoader for EmbeddedModuleLoader {
  #[instrument(level = "debug", skip(self))]
  fn resolve(
    &self,
    specifier: &str,
    referrer: &str,
    kind: ResolutionKind,
  ) -> Result<ModuleSpecifier, AnyError> {
    let referrer = if referrer == "." {
      if kind != ResolutionKind::MainModule {
        return Err(generic_error(format!(
          "Expected to resolve main module, got {:?} instead.",
          kind
        )));
      }

      let current_dir = std::env::current_dir().unwrap();
      deno_core::resolve_path(".", &current_dir)?
    } else {
      ModuleSpecifier::parse(referrer).map_err(|err| {
        type_error(format!("Referrer uses invalid specifier: {}", err))
      })?
    };
    let referrer_kind = if self
      .shared
      .cjs_tracker
      .is_maybe_cjs(&referrer, MediaType::from_specifier(&referrer))?
    {
      ResolutionMode::Require
    } else {
      ResolutionMode::Import
    };

    if self.shared.node_resolver.in_npm_package(&referrer) {
      return Ok(
        self
          .shared
          .node_resolver
          .resolve(
            specifier,
            &referrer,
            referrer_kind,
            NodeResolutionKind::Execution,
          )?
          .into_url(),
      );
    }

    let mapped_resolution =
      self.shared.workspace_resolver.resolve(specifier, &referrer);

    match mapped_resolution {
      Ok(MappedResolution::WorkspaceJsrPackage { specifier, .. }) => {
        Ok(specifier)
      }
      Ok(MappedResolution::WorkspaceNpmPackage {
        target_pkg_json: pkg_json,
        sub_path,
        ..
      }) => Ok(
        self
          .shared
          .node_resolver
          .resolve_package_subpath_from_deno_module(
            pkg_json.dir_path(),
            sub_path.as_deref(),
            Some(&referrer),
            referrer_kind,
            NodeResolutionKind::Execution,
          )?,
      ),
      Ok(MappedResolution::PackageJson {
        dep_result,
        sub_path,
        alias,
        ..
      }) => match dep_result.as_ref().map_err(|e| AnyError::from(e.clone()))? {
        PackageJsonDepValue::Req(req) => self
          .shared
          .npm_req_resolver
          .resolve_req_with_sub_path(
            req,
            sub_path.as_deref(),
            &referrer,
            referrer_kind,
            NodeResolutionKind::Execution,
          )
          .map_err(AnyError::from),

        PackageJsonDepValue::Workspace(version_req) => {
          let pkg_folder = self
            .shared
            .workspace_resolver
            .resolve_workspace_pkg_json_folder_for_pkg_json_dep(
              alias,
              version_req,
            )?;
          Ok(
            self
              .shared
              .node_resolver
              .resolve_package_subpath_from_deno_module(
                pkg_folder,
                sub_path.as_deref(),
                Some(&referrer),
                referrer_kind,
                NodeResolutionKind::Execution,
              )?,
          )
        }
      },
      Ok(MappedResolution::Normal { specifier, .. })
      | Ok(MappedResolution::ImportMap { specifier, .. }) => {
        if let Ok(reference) =
          NpmPackageReqReference::from_specifier(&specifier)
        {
          return Ok(self.shared.npm_req_resolver.resolve_req_reference(
            &reference,
            &referrer,
            referrer_kind,
            NodeResolutionKind::Execution,
          )?);
        }

        if specifier.scheme() == "jsr" {
          if let Some(module) = self.shared.eszip.get_module(&specifier) {
            return Ok(module.specifier);
          }
        }

        Ok(
          self
            .shared
            .node_resolver
            .handle_if_in_node_modules(&specifier)
            .unwrap_or(specifier),
        )
      }
      Err(err)
        if err.is_unmapped_bare_specifier() && referrer.scheme() == "file" =>
      {
        let maybe_res = self.shared.npm_req_resolver.resolve_if_for_npm_pkg(
          specifier,
          &referrer,
          referrer_kind,
          NodeResolutionKind::Execution,
        );
        if let Ok(Some(res)) = maybe_res {
          return Ok(res.into_url());
        }
        Err(err.into())
      }
      Err(err) => Err(err.into()),
    }
  }

  #[instrument(level = "debug", skip_all, fields(specifier = original_specifier.as_str()))]
  fn load(
    &self,
    original_specifier: &ModuleSpecifier,
    maybe_referrer: Option<&ModuleSpecifier>,
    _is_dynamic: bool,
    _requested_module_type: RequestedModuleType,
  ) -> deno_core::ModuleLoadResponse {
    let include_source_map = self.include_source_map;

    if original_specifier.scheme() == "data" {
      let data_url_text =
        match deno_graph::source::RawDataUrl::parse(original_specifier)
          .and_then(|url| url.decode())
        {
          Ok(response) => response,
          Err(err) => {
            return deno_core::ModuleLoadResponse::Sync(Err(type_error(
              format!("{:#}", err),
            )));
          }
        };

      return deno_core::ModuleLoadResponse::Sync(Ok(
        deno_core::ModuleSource::new(
          deno_core::ModuleType::JavaScript,
          ModuleSourceCode::String(data_url_text.into()),
          original_specifier,
          None,
        ),
      ));
    }

    if self.shared.node_resolver.in_npm_package(original_specifier) {
      let npm_module_loader = self.shared.npm_module_loader.clone();
      let original_specifier = original_specifier.clone();
      let maybe_referrer = maybe_referrer.cloned();

      return deno_core::ModuleLoadResponse::Async(
        async move {
          let code_source = npm_module_loader
            .load(&original_specifier, maybe_referrer.as_ref())
            .await?;

          Ok(deno_core::ModuleSource::new_with_redirect(
            match code_source.media_type {
              MediaType::Json => ModuleType::Json,
              _ => ModuleType::JavaScript,
            },
            code_source.code,
            &original_specifier,
            &code_source.found_url,
            None,
          ))
        }
        .boxed_local(),
      );
    }

    let Some(module) = self.shared.eszip.get_module(original_specifier) else {
      return deno_core::ModuleLoadResponse::Sync(Err(type_error(format!(
        "Module not found: {}",
        original_specifier
      ))));
    };

    let original_specifier = original_specifier.clone();

    deno_core::ModuleLoadResponse::Async(
      async move {
        let code = module.inner.source().await.ok_or_else(|| {
          type_error(format!("Module not found: {}", original_specifier))
        })?;

        let code = arc_u8_to_arc_str(code)
          .map_err(|_| type_error("Module source is not utf-8"))?;

        let source_map = module.inner.source_map().await;
        let maybe_code_with_source_map = 'scope: {
          if !include_source_map
            || !matches!(module.inner.kind, ModuleKind::JavaScript)
          {
            break 'scope code;
          }

          let Some(source_map) = source_map else {
            break 'scope code;
          };
          if source_map.is_empty() {
            break 'scope code;
          }

          let mut src = code.to_string();

          if !src.ends_with('\n') {
            src.push('\n');
          }

          const SOURCE_MAP_PREFIX: &str =
            "//# sourceMappingURL=data:application/json;base64,";

          src.push_str(SOURCE_MAP_PREFIX);

          base64::prelude::BASE64_STANDARD.encode_string(source_map, &mut src);
          Arc::from(src)
        };

        Ok(deno_core::ModuleSource::new_with_redirect(
          match module.inner.kind {
            ModuleKind::JavaScript => ModuleType::JavaScript,
            ModuleKind::Json => ModuleType::Json,
            ModuleKind::Jsonc => {
              return Err(type_error("jsonc modules not supported"))
            }
            ModuleKind::OpaqueData => {
              unreachable!();
            }
          },
          ModuleSourceCode::String(maybe_code_with_source_map.into()),
          &original_specifier,
          &module.specifier,
          None,
        ))
      }
      .boxed_local(),
    )
  }
}
