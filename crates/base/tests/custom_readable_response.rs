use std::thread::sleep;
use std::time::Duration;
use base::commands::start_server;
use tokio::select;

#[tokio::test]
async fn test_custom_readable_stream_response() {

    let req = async {
        sleep(Duration::from_secs(3));
        reqwest::get("http://localhost:8999/readable-stream-resp").await
    };

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
        resp = req => {
            assert_eq!(resp.unwrap().text().await.unwrap(), "Hello world from streams");
        }
    }
}
