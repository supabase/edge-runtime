use base::integration_test;

#[tokio::test]
async fn test_custom_readable_stream_response() {
    let none_req_builder: Option<reqwest::RequestBuilder> = None;
    integration_test!(
        "./test_cases/main",
        8999,
        "readable-stream-resp",
        none_req_builder,
        |resp: Result<reqwest::Response, reqwest::Error>| async {
            assert_eq!(
                resp.unwrap().text().await.unwrap(),
                "Hello world from streams"
            );
        }
    );
}
