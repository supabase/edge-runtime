use base::rt_worker::worker_ctx::{create_worker, WorkerRequestMsg};
use hyper::{Body, Request, Response};
use sb_worker_context::essentials::{
    UserWorkerRuntimeOpts, WorkerContextInitOpts, WorkerRuntimeOpts,
};
use std::collections::HashMap;
use tokio::sync::oneshot;

#[tokio::test]
async fn test_tls_throw_invalid_data() {
    let user_rt_opts = UserWorkerRuntimeOpts::default();
    let opts = WorkerContextInitOpts {
        service_path: "./test_cases/tls_invalid_data".into(),
        no_module_cache: false,
        import_map_path: None,
        env_vars: HashMap::new(),
        events_rx: None,
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

    assert_eq!(body_bytes, r#"{"passed":true}"#);
}
