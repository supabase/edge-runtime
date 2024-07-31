use http_v02::{header, response, HeaderMap, HeaderValue, Response, StatusCode};
use hyper_v014::body::Body;

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

pub fn emit_status_code(
    status: StatusCode,
    body: Option<Body>,
    connection_close: bool,
) -> Response<Body> {
    let builder = response::Builder::new().status(status);

    let builder = if connection_close {
        builder.header(header::CONNECTION, HeaderValue::from_static("close"))
    } else {
        builder
    };

    if let Some(body) = body {
        builder.body(body)
    } else {
        builder
            .header(http_v02::header::CONTENT_LENGTH, 0)
            .body(Body::empty())
    }
    .unwrap()
}
