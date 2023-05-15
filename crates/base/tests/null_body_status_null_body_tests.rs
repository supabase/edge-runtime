use base::commands::start_server;
use base::worker_ctx::{create_worker, WorkerRequestMsg};
use hyper::{Body, Request, Response};
use sb_worker_context::essentials::{EdgeContextInitOpts, EdgeContextOpts, EdgeUserRuntimeOpts};
use std::collections::HashMap;
use tokio::select;
use tokio::sync::oneshot;

#[tokio::test]
async fn test_null_body_with_204_status() {
    let user_rt_opts = EdgeUserRuntimeOpts::default();
    let opts = EdgeContextInitOpts {
        service_path: "./test_cases/empty-response".into(),
        no_module_cache: false,
        import_map_path: None,
        env_vars: HashMap::new(),
        conf: EdgeContextOpts::UserWorker(user_rt_opts),
    };
    let worker_req_tx = create_worker(opts).await.unwrap();
    let (res_tx, res_rx) = oneshot::channel::<Result<Response<Body>, hyper::Error>>();

    let req = Request::builder()
        .uri("/")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let msg = WorkerRequestMsg { req, res_tx };
    let _ = worker_req_tx.send(msg);

    let res = res_rx.await.unwrap().unwrap();
    assert!(res.status().as_u16() == 204);

    let body_bytes = hyper::body::to_bytes(res.into_body())
        .await
        .unwrap()
        .to_vec();

    assert_eq!(body_bytes.len(), 0);
}

#[tokio::test]
async fn test_null_body_with_204_status_post() {
    let user_rt_opts = EdgeUserRuntimeOpts::default();
    let opts = EdgeContextInitOpts {
        service_path: "./test_cases/empty-response".into(),
        no_module_cache: false,
        import_map_path: None,
        env_vars: HashMap::new(),
        conf: EdgeContextOpts::UserWorker(user_rt_opts),
    };
    let worker_req_tx = create_worker(opts).await.unwrap();
    let (res_tx, res_rx) = oneshot::channel::<Result<Response<Body>, hyper::Error>>();

    let req = Request::builder()
        .uri("/")
        .method("POST")
        .body(Body::empty())
        .unwrap();

    let msg = WorkerRequestMsg { req, res_tx };
    let _ = worker_req_tx.send(msg);

    let res = res_rx.await.unwrap().unwrap();
    assert!(res.status().as_u16() == 204);

    let body_bytes = hyper::body::to_bytes(res.into_body())
        .await
        .unwrap()
        .to_vec();

    assert_eq!(body_bytes.len(), 0);
}

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
