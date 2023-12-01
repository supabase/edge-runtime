use deno_ast::ModuleSpecifier;
use std::path::PathBuf;

/// Gets if the provided character is not supported on all
/// kinds of file systems.
pub fn is_banned_path_char(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*')
}

/// Gets a safe local directory name for the provided url.
///
/// For example:
/// https://deno.land:8080/path -> deno.land_8080/path
pub fn root_url_to_safe_local_dirname(root: &ModuleSpecifier) -> PathBuf {
    fn sanitize_segment(text: &str) -> String {
        text.chars()
            .map(|c| if is_banned_segment_char(c) { '_' } else { c })
            .collect()
    }

    fn is_banned_segment_char(c: char) -> bool {
        matches!(c, '/' | '\\') || is_banned_path_char(c)
    }

    let mut result = String::new();
    if let Some(domain) = root.domain() {
        result.push_str(&sanitize_segment(domain));
    }
    if let Some(port) = root.port() {
        if !result.is_empty() {
            result.push('_');
        }
        result.push_str(&port.to_string());
    }
    let mut result = PathBuf::from(result);
    if let Some(segments) = root.path_segments() {
        for segment in segments.filter(|s| !s.is_empty()) {
            result = result.join(sanitize_segment(segment));
        }
    }

    result
}
