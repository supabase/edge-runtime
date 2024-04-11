use deno_ast::ModuleSpecifier;
use std::path::{Path, PathBuf};

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

pub fn closest_common_directory(paths: &[String]) -> String {
    if paths.is_empty() {
        return String::new();
    }

    // Parse the URLs and collect the path components
    let paths: Vec<Vec<_>> = paths
        .iter()
        .map(|url| {
            let path = url.trim_start_matches("file://");
            Path::new(path)
                .components()
                .map(|comp| comp.as_os_str().to_str().unwrap_or(""))
                .collect()
        })
        .collect();

    // Find the shortest path to limit the comparison
    let min_length = paths.iter().map(|p| p.len()).min().unwrap_or(0);

    let mut common_path = Vec::new();

    for i in 0..min_length {
        let component = paths[0][i];
        if paths.iter().all(|path| path[i] == component) {
            common_path.push(component);
        } else {
            break;
        }
    }

    common_path.join("/")
}

pub fn find_lowest_path(paths: &Vec<String>) -> Option<String> {
    let mut lowest_path: Option<(&str, usize)> = None;

    for path_str in paths {
        // Extract the path part from the URL
        let path = Path::new(path_str);

        // Count the components
        let component_count = path.components().count();

        // Update the lowest path if this one has fewer components
        if lowest_path.is_none() || component_count < lowest_path.unwrap().1 {
            lowest_path = Some((path_str, component_count));
        }
    }

    lowest_path.map(|(path, _)| path.to_string())
}
