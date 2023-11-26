use base::rt_worker::worker_ctx::create_worker;
use hyper::{Body, Request, Response};
use sb_worker_context::essentials::{
    UserWorkerRuntimeOpts, WorkerContextInitOpts, WorkerRequestMsg, WorkerRuntimeOpts,
};
use std::collections::HashMap;
use tokio::sync::oneshot;

#[tokio::test]
async fn test_user_worker_json_imports() {
    let user_rt_opts = UserWorkerRuntimeOpts::default();
    let opts = WorkerContextInitOpts {
        service_path: "./test_cases/json_import".into(),
        no_module_cache: false,
        import_map_path: None,
        env_vars: HashMap::new(),
        events_rx: None,
        maybe_eszip: None,
        maybe_entrypoint: None,
        maybe_module_code: None,
        conf: WorkerRuntimeOpts::UserWorker(user_rt_opts),
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
    assert!(res.status().as_u16() == 200);

    let body_bytes = hyper::body::to_bytes(res.into_body()).await.unwrap();

    assert_eq!(body_bytes, r#"{"version":"1.0.0"}"#);
}

#[tokio::test]
async fn test_user_imports_npm() {
    let user_rt_opts = UserWorkerRuntimeOpts::default();
    let opts = WorkerContextInitOpts {
        service_path: "./test_cases/npm".into(),
        no_module_cache: false,
        import_map_path: None,
        env_vars: HashMap::new(),
        events_rx: None,
        maybe_eszip: None,
        maybe_entrypoint: None,
        maybe_module_code: None,
        conf: WorkerRuntimeOpts::UserWorker(user_rt_opts),
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
    assert!(res.status().as_u16() == 200);

    let body_bytes = hyper::body::to_bytes(res.into_body()).await.unwrap();

    assert_eq!(body_bytes, r#"{"version":true}"#);
}
