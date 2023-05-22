use base::integration_test;

// NOTE: Only add user worker tests that's using oak server here.
// Any other user worker tests, add to `user_worker_tests.rs`.

#[tokio::test]
async fn test_oak_server() {
    let none_req_builder: Option<reqwest::RequestBuilder> = None;
    integration_test!(
        "./test_cases/oak",
        8999,
        "oak",
        none_req_builder,
        |resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            println!("{}", res.status().as_u16());
            assert!(res.status().as_u16() == 200);
            assert_eq!(
                res.bytes().await.unwrap(),
                "This is an example Oak server running on Edge Functions!"
            );
        }
    );
}

#[tokio::test]
async fn test_file_upload() {
    let body_chunk = "--TEST\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\nContent-Type: text/plain\r\n\r\ntestuser\r\n--TEST--\r\n";
    let content_length = &body_chunk.len();
    let chunks: Vec<Result<_, std::io::Error>> = vec![Ok(body_chunk)];
    let stream = futures_util::stream::iter(chunks);
    let body = reqwest::Body::wrap_stream(stream);

    let req = reqwest::Client::new()
        .request(reqwest::Method::POST, "http://localhost:8999/file-upload")
        .body(body)
        .header("Content-Type", "multipart/form-data; boundary=TEST")
        .header("Content-Length", content_length.to_string());

    integration_test!(
        "./test_cases/oak",
        8999,
        "file-upload",
        Some(req),
        |resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 201);
            assert_eq!(res.bytes().await.unwrap(), "file-type: text/plain");
        }
    );
}
