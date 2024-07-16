use deno_ast::ParseDiagnostic;
use deno_core::error::AnyError;
use deno_graph::{ModuleError, ModuleGraphError};
use deno_graph::{ModuleLoadError, ResolutionError};
use import_map::ImportMapError;

fn get_import_map_error_class(_: &ImportMapError) -> &'static str {
    "URIError"
}

fn get_diagnostic_class(_: &ParseDiagnostic) -> &'static str {
    "SyntaxError"
}

fn get_module_graph_error_class(err: &ModuleGraphError) -> &'static str {
    use deno_graph::JsrLoadError;
    use deno_graph::NpmLoadError;
    use deno_graph::WorkspaceLoadError;

    match err {
        ModuleGraphError::ResolutionError(err) | ModuleGraphError::TypesResolutionError(err) => {
            get_resolution_error_class(err)
        }

        ModuleGraphError::ModuleError(err) => {
            match err {
                ModuleError::InvalidTypeAssertion { .. } => "SyntaxError",
                ModuleError::ParseErr(_, diagnostic) => get_diagnostic_class(diagnostic),
                ModuleError::UnsupportedMediaType { .. }
                | ModuleError::UnsupportedImportAttributeType { .. } => "TypeError",
                ModuleError::Missing(_, _) | ModuleError::MissingDynamic(_, _) => "NotFound",

                ModuleError::LoadingErr(_, _, err) => match err {
                    ModuleLoadError::Loader(err) => get_error_class_name(err.as_ref()),
                    ModuleLoadError::HttpsChecksumIntegrity(_)
                    | ModuleLoadError::TooManyRedirects => "Error",
                    ModuleLoadError::NodeUnknownBuiltinModule(_) => "NotFound",
                    ModuleLoadError::Decode(_) => "TypeError",
                    ModuleLoadError::Npm(err) => match err {
                        NpmLoadError::NotSupportedEnvironment
                        | NpmLoadError::PackageReqResolution(_)
                        | NpmLoadError::RegistryInfo(_) => "Error",
                        NpmLoadError::PackageReqReferenceParse(_) => "TypeError",
                    },
                    ModuleLoadError::Jsr(err) => match err {
                        JsrLoadError::UnsupportedManifestChecksum
                        | JsrLoadError::PackageFormat(_) => "TypeError",
                        JsrLoadError::ContentLoadExternalSpecifier
                        | JsrLoadError::ContentLoad(_)
                        | JsrLoadError::ContentChecksumIntegrity(_)
                        | JsrLoadError::PackageManifestLoad(_, _)
                        | JsrLoadError::PackageVersionManifestChecksumIntegrity(..)
                        | JsrLoadError::PackageVersionManifestLoad(_, _)
                        | JsrLoadError::RedirectInPackage(_) => "Error",
                        JsrLoadError::PackageNotFound(_)
                        | JsrLoadError::PackageReqNotFound(_)
                        | JsrLoadError::PackageVersionNotFound(_)
                        | JsrLoadError::UnknownExport { .. } => "NotFound",
                    },
                    ModuleLoadError::Workspace(err) => match err {
                        WorkspaceLoadError::MemberInvalidExportPath { .. } => "TypeError",
                        WorkspaceLoadError::MissingMemberExports { .. } => "NotFound",
                    },
                },
            }
        }
    }
}

fn get_resolution_error_class(err: &ResolutionError) -> &'static str {
    match err {
        ResolutionError::ResolverError { error, .. } => {
            use deno_graph::source::ResolveError::*;
            match error.as_ref() {
                Specifier(_) => "TypeError",
                Other(e) => get_error_class_name(e),
            }
        }
        _ => "TypeError",
    }
}

pub fn get_error_class_name(e: &AnyError) -> &'static str {
    #[allow(clippy::format_collect)]
    deno_core::error::get_custom_error_class(e)
        .or_else(|| {
            e.downcast_ref::<ImportMapError>()
                .map(get_import_map_error_class)
        })
        .or_else(|| {
            e.downcast_ref::<ParseDiagnostic>()
                .map(get_diagnostic_class)
        })
        .or_else(|| {
            e.downcast_ref::<ModuleGraphError>()
                .map(get_module_graph_error_class)
        })
        .or_else(|| {
            e.downcast_ref::<ResolutionError>()
                .map(get_resolution_error_class)
        })
        .unwrap_or_else(|| {
            eprintln!(
                "Error '{}' contains boxed error of unknown type:{}",
                e,
                e.chain()
                    .map(|e| format!("\n  {:?}", e))
                    .collect::<String>()
            );
            "Error"
        })
}
