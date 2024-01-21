#[path = "../src/utils/integration_test_helper.rs"]
mod integration_test_helper;

use hyper::{Body, Request, Response};
use sb_workers::context::{WorkerContextInitOpts, WorkerRequestMsg, WorkerRuntimeOpts};
use std::collections::HashMap;
use tokio::sync::oneshot;

use crate::integration_test_helper::{create_test_user_worker, test_user_runtime_opts};

#[tokio::test]
async fn test_tls_throw_invalid_data() {
    let opts = WorkerContextInitOpts {
        service_path: "./test_cases/tls_invalid_data".into(),
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

    let req = Request::builder()
        .uri("/")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let conn_watch = scope.conn_rx();
    let req_guard = scope.start_request().await;
    let msg = WorkerRequestMsg {
        req,
        res_tx,
        conn_watch,
    };

    let _ = worker_req_tx.send(msg);

    let res = res_rx.await.unwrap().unwrap();
    assert!(res.status().as_u16() == 200);

    let body_bytes = hyper::body::to_bytes(res.into_body()).await.unwrap();

    assert_eq!(body_bytes, r#"{"passed":true}"#);
    req_guard.await;
}
