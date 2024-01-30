#[path = "../src/utils/integration_test_helper.rs"]
mod integration_test_helper;

use base::integration_test;
use http::Method;
use hyper::Body;
use serial_test::serial;

// NOTE: Only add user worker tests that's using oak server here.
// Any other user worker tests, add to `user_worker_tests.rs`.

#[tokio::test]
#[serial]
async fn test_oak_server() {
    let port = 8798;
    integration_test!(
        "./test_cases/oak",
        port,
        "oak",
        None,
        None,
        None::<reqwest::RequestBuilder>,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 200);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(
                body_bytes,
                "This is an example Oak server running on Edge Functions!"
            );
        })
    );
}

#[tokio::test]
#[serial]
async fn test_file_upload() {
    let body_chunk = "--TEST\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\nContent-Type: text/plain\r\n\r\ntestuser\r\n--TEST--\r\n";

    let content_length = &body_chunk.len();
    let chunks: Vec<Result<_, std::io::Error>> = vec![Ok(body_chunk)];
    let stream = futures_util::stream::iter(chunks);
    let body = Body::wrap_stream(stream);

    let port = 8788;
    let client = reqwest::Client::new();
    let req = client
        .request(
            Method::POST,
            format!("http://localhost:{}/file-upload", port),
        )
        .header("Content-Type", "multipart/form-data; boundary=TEST")
        .header("Content-Length", content_length.to_string())
        .body(body)
        .build()
        .unwrap();

    let original = reqwest::RequestBuilder::from_parts(client, req);

    let request_builder = Some(original);

    integration_test!(
        "./test_cases/oak",
        port,
        "",
        None,
        None,
        request_builder,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 201);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(body_bytes, "file-type: text/plain");
        })
    );
}
