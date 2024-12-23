use deno_ast::ModuleSpecifier;
use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

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

pub fn get_atomic_dir_path(file_path: &Path) -> PathBuf {
    let rand = gen_rand_path_component();
    let new_file_name = format!(
        ".{}_{}",
        file_path
            .file_name()
            .map(|f| f.to_string_lossy())
            .unwrap_or(Cow::Borrowed("")),
        rand
    );
    file_path.with_file_name(new_file_name)
}

fn gen_rand_path_component() -> String {
    (0..4).fold(String::new(), |mut output, _| {
        output.push_str(&format!("{:02x}", rand::random::<u8>()));
        output
    })
}
