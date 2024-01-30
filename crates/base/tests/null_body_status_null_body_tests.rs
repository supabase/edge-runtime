#[path = "../src/utils/integration_test_helper.rs"]
mod integration_test_helper;

use base::integration_test;
use http::Method;
use hyper::Body;
use serial_test::serial;

#[tokio::test]
#[serial]
async fn test_null_body_with_204_status() {
    let port = 8878;
    integration_test!(
        "./test_cases/empty-response",
        port,
        "",
        None,
        None,
        None::<reqwest::RequestBuilder>,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 204);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(body_bytes.len(), 0);
        })
    );
}

#[tokio::test]
#[serial]
async fn test_null_body_with_204_status_post() {
    let port = 8888;
    let client = reqwest::Client::new();
    let req = client
        .request(Method::POST, format!("http://localhost:{}", port))
        .body(Body::empty())
        .build()
        .unwrap();

    let original = reqwest::RequestBuilder::from_parts(client, req);

    let request_builder = Some(original);

    integration_test!(
        "./test_cases/empty-response",
        port,
        "",
        None,
        None,
        request_builder,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 204);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(body_bytes.len(), 0);
        })
    );
}
