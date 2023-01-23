use deno_core::error::uri_error;
use deno_core::error::AnyError;
use deno_core::ModuleSpecifier;

#[derive(Default, Clone, Debug)]
pub struct Permissions;

impl Permissions {
    /// A helper function that determines if the module specifier is a local or
    /// remote, and performs a read or net check for the specifier.
    pub fn check_specifier(&mut self, specifier: &ModuleSpecifier) -> Result<(), AnyError> {
        match specifier.scheme() {
            "file" => match specifier.to_file_path() {
                // allow all file paths
                Ok(_) => Ok(()),
                Err(_) => Err(uri_error(format!(
                    "Invalid file path.\n  Specifier: {}",
                    specifier
                ))),
            },
            "data" => Ok(()),
            "blob" => Ok(()),
            // allow remote modules
            _ => Ok(()),
        }
    }
}
