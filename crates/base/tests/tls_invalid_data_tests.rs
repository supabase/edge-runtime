use base::integration_test;
use flaky_test::flaky_test;

#[flaky_test]
async fn test_tls_throw_invalid_data() {
    let none_req_builder: Option<reqwest::RequestBuilder> = None;
    integration_test!(
        "./test_cases/tls_invalid_data",
        2001,
        "/",
        none_req_builder,
        |resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 200);
            assert_eq!(res.bytes().await.unwrap(), r#"{"passed":true}"#);
        }
    );
}
