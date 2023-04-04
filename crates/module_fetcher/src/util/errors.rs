use deno_ast::Diagnostic;
use deno_core::error::AnyError;
use deno_graph::{ModuleError, ModuleGraphError};
use deno_graph::ResolutionError;
use import_map::ImportMapError;

fn get_import_map_error_class(_: &ImportMapError) -> &'static str {
    "URIError"
}

fn get_diagnostic_class(_: &Diagnostic) -> &'static str {
    "SyntaxError"
}

fn get_module_graph_error_class(err: &ModuleGraphError) -> &'static str {
    match err {
        ModuleGraphError::ModuleError(err) => match err {
            ModuleError::LoadingErr(_, _, error) =>{
                "Loading Err"
            },
            ModuleError::Missing(_, _) => "NotFound",
            ModuleError::MissingDynamic(_, _) => "Unknown",
            ModuleError::ParseErr(_, diag) => get_diagnostic_class(&diag),
            ModuleError::InvalidTypeAssertion { .. } => "SyntaxError",
            ModuleError::UnsupportedMediaType(_, _, _) | ModuleError::UnsupportedImportAssertionType { .. } => "TypeError"
        }
        ModuleGraphError::ResolutionError(err) => get_resolution_error_class(err),
    }
}

fn get_resolution_error_class(err: &ResolutionError) -> &'static str {
    match err {
        ResolutionError::ResolverError { error, .. } => get_error_class_name(error.as_ref()),
        _ => "TypeError",
    }
}

pub fn get_error_class_name(e: &AnyError) -> &'static str {
    deno_core::error::get_custom_error_class(e)
        .or_else(|| {
            e.downcast_ref::<ImportMapError>()
                .map(get_import_map_error_class)
        })
        .or_else(|| e.downcast_ref::<Diagnostic>().map(get_diagnostic_class))
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
