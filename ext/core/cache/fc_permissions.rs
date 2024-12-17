use deno_core::error::uri_error;
use deno_core::error::AnyError;
use deno_core::ModuleSpecifier;
use std::path::PathBuf;

#[derive(Default, Clone, Debug)]
pub struct FcPermissions {
    root_path: PathBuf,
}

impl FcPermissions {
    pub fn new(root_path: PathBuf) -> Self {
        Self { root_path }
    }

    /// A helper function that determines if the module specifier is a local or
    /// remote, and performs a read or net check for the specifier.
    pub fn check_specifier(&mut self, specifier: &ModuleSpecifier) -> Result<(), AnyError> {
        match specifier.scheme() {
            "file" => match specifier.to_file_path() {
                Ok(file_path) => {
                    if !file_path.starts_with(&self.root_path) {
                        return Err(uri_error(format!(
                            "Invalid file path.\n  Specifier: {}",
                            specifier
                        )));
                    }
                    Ok(())
                }
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

    // TODO: audit the API calling allow_all()
    pub fn allow_all() -> Self {
        Self {
            root_path: PathBuf::new(),
        }
    }
}
