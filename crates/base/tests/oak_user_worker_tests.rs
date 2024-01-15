#[path = "../src/utils/integration_test_helper.rs"]
mod integration_test_helper;

use hyper::{Body, Request, Response};
use sb_workers::context::{WorkerContextInitOpts, WorkerRequestMsg, WorkerRuntimeOpts};
use serial_test::serial;
use std::collections::HashMap;
use tokio::sync::oneshot;

use crate::integration_test_helper::{create_test_user_worker, test_user_runtime_opts};

// NOTE: Only add user worker tests that's using oak server here.
// Any other user worker tests, add to `user_worker_tests.rs`.

#[tokio::test]
#[serial]
async fn test_oak_server() {
    let opts = WorkerContextInitOpts {
        service_path: "./test_cases/oak".into(),
        no_module_cache: false,
        import_map_path: None,
        env_vars: HashMap::new(),
        events_rx: None,
        timing: None,
        maybe_eszip: None,
        maybe_entrypoint: None,
        maybe_module_code: None,
        conf: WorkerRuntimeOpts::UserWorker(test_user_runtime_opts()),
    };

    let (worker_req_tx, scope) = create_test_user_worker(opts).await.unwrap();
    let (res_tx, res_rx) = oneshot::channel::<Result<Response<Body>, hyper::Error>>();

    let conn_watch = scope.conn_rx();
    let req_guard = scope.start_request().await;
    let req = Request::builder()
        .uri("/oak")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let msg = WorkerRequestMsg {
        req,
        res_tx,
        conn_watch,
    };

    let _ = worker_req_tx.send(msg);

    let res = res_rx.await.unwrap().unwrap();
    assert!(res.status().as_u16() == 200);

    let body_bytes = hyper::body::to_bytes(res.into_body()).await.unwrap();

    assert_eq!(
        body_bytes,
        "This is an example Oak server running on Edge Functions!"
    );

    req_guard.await;
}

#[tokio::test]
#[serial]
async fn test_file_upload() {
    let opts = WorkerContextInitOpts {
        service_path: "./test_cases/oak".into(),
        no_module_cache: false,
        import_map_path: None,
        env_vars: HashMap::new(),
        events_rx: None,
        timing: None,
        maybe_eszip: None,
        maybe_entrypoint: None,
        maybe_module_code: None,
        conf: WorkerRuntimeOpts::UserWorker(test_user_runtime_opts()),
    };

    let (worker_req_tx, scope) = create_test_user_worker(opts).await.unwrap();
    let (res_tx, res_rx) = oneshot::channel::<Result<Response<Body>, hyper::Error>>();

    let body_chunk = "--TEST\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\nContent-Type: text/plain\r\n\r\ntestuser\r\n--TEST--\r\n";

    let content_length = &body_chunk.len();
    let chunks: Vec<Result<_, std::io::Error>> = vec![Ok(body_chunk)];
    let stream = futures_util::stream::iter(chunks);
    let body = Body::wrap_stream(stream);

    let conn_watch = scope.conn_rx();
    let req_guard = scope.start_request().await;
    let req = Request::builder()
        .uri("/file-upload")
        .method("POST")
        .header("Content-Type", "multipart/form-data; boundary=TEST")
        .header("Content-Length", content_length.to_string())
        .body(body)
        .unwrap();

    let msg = WorkerRequestMsg {
        req,
        res_tx,
        conn_watch,
    };

    let _ = worker_req_tx.send(msg);

    let res = res_rx.await.unwrap().unwrap();
    assert!(res.status().as_u16() == 201);

    let body_bytes = hyper::body::to_bytes(res.into_body()).await.unwrap();

    assert_eq!(body_bytes, "file-type: text/plain");
    req_guard.await;
}
