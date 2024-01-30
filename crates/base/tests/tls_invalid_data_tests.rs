#[path = "../src/utils/integration_test_helper.rs"]
mod integration_test_helper;

use base::integration_test;

#[tokio::test]
async fn test_tls_throw_invalid_data() {
    let port = 8598;
    integration_test!(
        "./test_cases/tls_invalid_data",
        port,
        "",
        None,
        None,
        None::<reqwest::RequestBuilder>,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 200);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(body_bytes, r#"{"passed":true}"#);
        })
    );
}
