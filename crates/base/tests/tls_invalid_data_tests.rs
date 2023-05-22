use base::integration_test;
mod common;
#[tokio::test]
async fn test_tls_throw_invalid_data() {
    let port = common::port_picker::get_available_port();
    let none_req_builder: Option<reqwest::RequestBuilder> = None;
    integration_test!(
        "./test_cases/tls_invalid_data",
        port,
        "/",
        none_req_builder,
        |resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 200);
            assert_eq!(res.bytes().await.unwrap(), r#"{"passed":true}"#);
        }
    );
}
