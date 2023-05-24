use base::integration_test;
use flaky_test::flaky_test;

#[flaky_test]
async fn test_null_body_with_204_status() {
    let port = 2005_u16;
    let none_req_builder: Option<reqwest::RequestBuilder> = None;
    integration_test!(
        "./test_cases/empty-response",
        port,
        "/",
        none_req_builder,
        |resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 204);
            assert_eq!(res.bytes().await.unwrap().len(), 0);
        }
    );
}

#[flaky_test]
async fn test_null_body_with_204_status_post() {
    let port = 2004_u16;
    integration_test!(
        "./test_cases/empty-response",
        port,
        "/",
        Some(
            reqwest::Client::new()
                .request(
                    reqwest::Method::POST,
                    format!("http://localhost:{}/std_user_worker", port)
                )
                .body(reqwest::Body::from(vec![]))
        ),
        |resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 204);
            assert_eq!(res.bytes().await.unwrap().len(), 0);
        }
    );
}
