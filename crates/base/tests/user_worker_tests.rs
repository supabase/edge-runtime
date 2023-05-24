use base::integration_test;
use flaky_test::flaky_test;

#[flaky_test]
async fn test_json_imports() {
    let none_req_builder: Option<reqwest::RequestBuilder> = None;
    integration_test!(
        "./test_cases/json_import",
        2000,
        "/",
        none_req_builder,
        |resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 200);
            assert_eq!(res.bytes().await.unwrap(), r#"{"version":"1.0.0"}"#);
        }
    );
}
