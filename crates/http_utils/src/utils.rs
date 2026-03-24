use http_v02::header;
use http_v02::response;
use http_v02::HeaderMap;
use http_v02::HeaderValue;
use http_v02::Response;
use http_v02::StatusCode;
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

#[cfg(test)]
mod test {
  use super::*;

  #[test]
  fn get_upgrade_type_returns_none_without_headers() {
    let headers = HeaderMap::new();
    assert!(get_upgrade_type(&headers).is_none());
  }

  #[test]
  fn get_upgrade_type_returns_none_without_connection_upgrade() {
    let mut headers = HeaderMap::new();
    headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
    // Missing Connection: Upgrade
    assert!(get_upgrade_type(&headers).is_none());
  }

  #[test]
  fn get_upgrade_type_returns_upgrade_value() {
    let mut headers = HeaderMap::new();
    headers.insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
    headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
    assert_eq!(
      get_upgrade_type(&headers),
      Some("websocket".to_string())
    );
  }

  #[test]
  fn get_upgrade_type_handles_comma_separated_connection() {
    let mut headers = HeaderMap::new();
    headers.insert(
      header::CONNECTION,
      HeaderValue::from_static("keep-alive, Upgrade"),
    );
    headers.insert(header::UPGRADE, HeaderValue::from_static("h2c"));
    assert_eq!(get_upgrade_type(&headers), Some("h2c".to_string()));
  }

  #[test]
  fn emit_status_code_with_empty_body() {
    let resp = emit_status_code(StatusCode::OK, None, false);
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
      resp.headers().get(header::CONTENT_LENGTH).unwrap(),
      "0"
    );
  }

  #[test]
  fn emit_status_code_with_connection_close() {
    let resp = emit_status_code(StatusCode::BAD_GATEWAY, None, true);
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
      resp.headers().get(header::CONNECTION).unwrap(),
      "close"
    );
  }

  #[test]
  fn emit_status_code_with_body() {
    let body = Body::from("error message");
    let resp = emit_status_code(StatusCode::INTERNAL_SERVER_ERROR, Some(body), false);
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    // Should not have content-length: 0 when body is provided
    assert!(resp.headers().get(header::CONTENT_LENGTH).is_none());
  }
}
