// (transform_jsx, jsx_automatic, jsx_development, precompile_jsx)
pub fn get_jsx_emit_opts(jsx: &str) -> (bool, bool, bool, bool) {
    match jsx {
        "react" => (true, false, false, false),
        "react-jsx" => (true, true, false, false),
        "react-jsxdev" => (true, true, true, false),
        "precompile" => (false, false, false, true),
        _ => (false, false, false, false),
    }
}

pub fn get_jsx_rt(jsx: &str) -> Option<String> {
    match jsx {
        "react-jsx" => Some("jsx-runtime".to_string()),
        "react-jsxdev" => Some("jsx-dev-runtime".to_string()),
        "precompile" => Some("jsx-runtime".to_string()),
        "react" => None,
        _ => Some(jsx.to_string()),
    }
}

pub fn get_rt_from_jsx(rt: Option<String>) -> String {
    match rt.unwrap_or("none".to_string()).as_ref() {
        "jsx-runtime" => "react-jsx".to_string(),
        "jsx-dev-runtime" => "react-jsxdev".to_string(),
        "precompile" => "jsx-runtime".to_string(),
        _ => "react".to_string(),
    }
}
