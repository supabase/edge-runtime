#[path = "../src/utils/integration_test_helper.rs"]
mod integration_test_helper;

use base::integration_test;
use serial_test::serial;

#[tokio::test]
#[serial]
async fn test_user_worker_json_imports() {
    let port = 8498;
    integration_test!(
        "./test_cases/json_import",
        port,
        "",
        None,
        None,
        None::<reqwest::RequestBuilder>,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 200);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(body_bytes, r#"{"version":"1.0.0"}"#);
        })
    );
}

#[tokio::test]
#[serial]
async fn test_user_imports_npm() {
    let port = 8488;
    integration_test!(
        "./test_cases/npm",
        port,
        "",
        None,
        None,
        None::<reqwest::RequestBuilder>,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 200);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(
                body_bytes,
                r#"{"is_even":true,"hello":"","numbers":{"Uno":1,"Dos":2}}"#
            );
        })
    );
}
