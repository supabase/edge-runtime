use base::worker_ctx::{create_worker, WorkerRequestMsg};
use hyper::{Body, Request, Response};
use sb_worker_context::essentials::{EdgeContextInitOpts, EdgeContextOpts, EdgeUserRuntimeOpts};
use std::collections::HashMap;
use tokio::sync::oneshot;

// NOTE: Only add user worker tests that's using oak server here.
// Any other user worker tests, add to `user_worker_tests.rs`.

#[tokio::test]
async fn test_oak_server() {
    let user_rt_opts = EdgeUserRuntimeOpts::default();
    let opts = EdgeContextInitOpts {
        service_path: "./test_cases/oak".into(),
        no_module_cache: false,
        import_map_path: None,
        env_vars: HashMap::new(),
        conf: EdgeContextOpts::UserWorker(user_rt_opts),
    };
    let worker_req_tx = create_worker(opts, None).await.unwrap();
    let (res_tx, res_rx) = oneshot::channel::<Result<Response<Body>, hyper::Error>>();

    let req = Request::builder()
        .uri("/oak")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let msg = WorkerRequestMsg { req, res_tx };
    let _ = worker_req_tx.send(msg);

    let res = res_rx.await.unwrap().unwrap();
    assert!(res.status().as_u16() == 200);

    let body_bytes = hyper::body::to_bytes(res.into_body()).await.unwrap();

    assert_eq!(
        body_bytes,
        "This is an example Oak server running on Edge Functions!"
    );
}

#[tokio::test]
async fn test_file_upload() {
    let user_rt_opts = EdgeUserRuntimeOpts::default();
    let opts = EdgeContextInitOpts {
        service_path: "./test_cases/oak".into(),
        no_module_cache: false,
        import_map_path: None,
        env_vars: HashMap::new(),
        conf: EdgeContextOpts::UserWorker(user_rt_opts),
    };
    let worker_req_tx = create_worker(opts, None).await.unwrap();
    let (res_tx, res_rx) = oneshot::channel::<Result<Response<Body>, hyper::Error>>();

    let body_chunk = "--TEST\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\nContent-Type: text/plain\r\n\r\ntestuser\r\n--TEST--\r\n";

    let content_length = &body_chunk.len();
    let chunks: Vec<Result<_, std::io::Error>> = vec![Ok(body_chunk)];
    let stream = futures_util::stream::iter(chunks);
    let body = Body::wrap_stream(stream);

    let req = Request::builder()
        .uri("/file-upload")
        .method("POST")
        .header("Content-Type", "multipart/form-data; boundary=TEST")
        .header("Content-Length", content_length.to_string())
        .body(body)
        .unwrap();

    let msg = WorkerRequestMsg { req, res_tx };
    let _ = worker_req_tx.send(msg);

    let res = res_rx.await.unwrap().unwrap();
    assert!(res.status().as_u16() == 201);

    let body_bytes = hyper::body::to_bytes(res.into_body()).await.unwrap();

    assert_eq!(body_bytes, "file-type: text/plain");
}
