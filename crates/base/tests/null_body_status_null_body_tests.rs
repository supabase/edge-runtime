use base::integration_test;

#[tokio::test]
async fn test_null_body_with_204_status() {
    let none_req_builder: Option<reqwest::RequestBuilder> = None;
    integration_test!(
        "./test_cases/empty-response",
        8999,
        "/",
        none_req_builder,
        |resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 204);
            assert_eq!(res.bytes().await.unwrap().len(), 0);
        }
    );
}

#[tokio::test]
async fn test_null_body_with_204_status_post() {
    integration_test!(
        "./test_cases/empty-response",
        8999,
        "/",
        Some(
            reqwest::Client::new()
                .request(
                    reqwest::Method::POST,
                    "http://localhost:8999/std_user_worker"
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
