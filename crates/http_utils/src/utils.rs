use http::{header, response, HeaderMap, Response, StatusCode};
use hyper::Body;

pub fn get_upgrade_type(headers: &HeaderMap) -> Option<String> {
    let connection_header_exists = headers
        .get(header::CONNECTION)
        .map(|it| {
            it.to_str()
                .unwrap_or("")
                .split(',')
                .any(|str| str.trim() == header::UPGRADE)
        })
        .unwrap_or(false);

    if connection_header_exists {
        if let Some(upgrade) = headers.get(header::UPGRADE) {
            return upgrade.to_str().ok().map(str::to_owned);
        }
    }

    None
}

pub fn emit_status_code(status: StatusCode) -> Response<Body> {
    response::Builder::new()
        .status(status)
        .body(Body::empty())
        .unwrap()
}
