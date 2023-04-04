use std::path::PathBuf;
use deno_core::ModuleSpecifier;
use crate::util::path::specifier_to_file_path;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FilesConfig {
    pub include: Vec<PathBuf>,
    pub exclude: Vec<PathBuf>,
}

impl FilesConfig {
    /// Gets if the provided specifier is allowed based on the includes
    /// and excludes in the configuration file.
    pub fn matches_specifier(&self, specifier: &ModuleSpecifier) -> bool {
        let file_path = match specifier_to_file_path(specifier) {
            Ok(file_path) => file_path,
            Err(_) => return false,
        };
        // Skip files which is in the exclude list.
        if self.exclude.iter().any(|i| file_path.starts_with(i)) {
            return false;
        }

        // Ignore files not in the include list if it's not empty.
        self.include.is_empty()
            || self.include.iter().any(|i| file_path.starts_with(i))
    }
}