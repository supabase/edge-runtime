extern crate core;

use base::integration_test;

#[tokio::test]
async fn test_custom_readable_stream_response() {
    integration_test!(
        "./test_cases/main",
        8999,
        "readable-stream-resp",
        None,
        None,
        None::<reqwest::RequestBuilder>,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            assert_eq!(
                resp.unwrap().text().await.unwrap(),
                "Hello world from streams"
            );
        })
    );
}
