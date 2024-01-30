#[path = "../src/utils/integration_test_helper.rs"]
mod integration_test_helper;

use base::integration_test;
use serial_test::serial;
use std::path::Path;
use urlencoding::encode;

#[tokio::test]
#[serial]
async fn test_import_map_file_path() {
    integration_test!(
        "./test_cases/with_import_map",
        8989,
        "",
        None,
        Some("./test_cases/with_import_map/import_map.json".to_string()),
        None::<reqwest::RequestBuilder>,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 200);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(body_bytes, r#"{"message":"ok"}"#);
        })
    );
}

#[tokio::test]
#[serial]
async fn test_import_map_inline() {
    let inline_import_map = format!(
        "data:{}?{}",
        encode(
            r#"{
        "imports": {
            "std/": "https://deno.land/std@0.131.0/",
            "foo/": "./bar/"
        }
    }"#
        ),
        encode(
            Path::new(&std::env::current_dir().unwrap())
                .join("./test_cases/with_import_map")
                .to_str()
                .unwrap()
        )
    );

    integration_test!(
        "./test_cases/with_import_map",
        8978,
        "",
        None,
        Some(inline_import_map),
        None::<reqwest::RequestBuilder>,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 200);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(body_bytes, r#"{"message":"ok"}"#);
        })
    );
}
