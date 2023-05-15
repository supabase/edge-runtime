use base::commands::start_server;
use tokio::select;

#[tokio::test]
async fn test_custom_readable_stream_response() {
    select! {
        _ = start_server(
            "0.0.0.0",
            8999,
            String::from("./test_cases/main"),
            None,
            false
        ) => {
            panic!("This one should not end first");
        }
        resp = reqwest::get("http://localhost:8999/readable-stream-resp") => {
            assert_eq!(resp.unwrap().text().await.unwrap(), "Hello world from streams");
        }
    }
}
