use import_map::ImportMapDiagnostic;
use log::warn;

pub mod cache_setting;
pub mod checksum;
pub mod config_file;
pub mod errors;
pub mod fs;
pub mod path;
pub mod text_encoding;
pub mod version;

pub fn print_import_map_diagnostics(diagnostics: &[ImportMapDiagnostic]) {
    if !diagnostics.is_empty() {
        warn!(
            "Import map diagnostics:\n{}",
            diagnostics
                .iter()
                .map(|d| format!("  - {d}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}