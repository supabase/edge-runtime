#[path = "../src/utils/integration_test_helper.rs"]
mod integration_test_helper;

use hyper::{Body, Request, Response};
use std::collections::HashMap;
use std::path::Path;
use tokio::sync::oneshot;
use urlencoding::encode;

use base::rt_worker::worker_ctx::create_worker;
use sb_workers::context::{
    UserWorkerRuntimeOpts, WorkerContextInitOpts, WorkerRequestMsg, WorkerRuntimeOpts,
};

#[tokio::test]
async fn test_import_map_file_path() {
    let user_rt_opts = UserWorkerRuntimeOpts::default();
    let opts = WorkerContextInitOpts {
        service_path: "./test_cases/with_import_map".into(),
        no_module_cache: false,
        import_map_path: Some("./test_cases/with_import_map/import_map.json".to_string()),
        env_vars: HashMap::new(),
        events_rx: None,
        timing: None,
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

    let (_conn_tx, conn_rx) = integration_test_helper::create_conn_watch();
    let msg = WorkerRequestMsg {
        req,
        res_tx,
        conn_watch: Some(conn_rx),
    };

    let _ = worker_req_tx.send(msg);

    let res = res_rx.await.unwrap().unwrap();
    assert!(res.status().as_u16() == 200);

    let body_bytes = hyper::body::to_bytes(res.into_body()).await.unwrap();

    assert_eq!(body_bytes, r#"{"message":"ok"}"#);
}

#[tokio::test]
async fn test_import_map_inline() {
    let user_rt_opts = UserWorkerRuntimeOpts::default();
    let inline_import_map = format!(
        "data:{}?{}",
        encode(
            r#"{
        "imports": {
            "std/": "https://deno.land/std@0.131.0/",
            "foo/": "./bar/"
        }
    }"#
        ),
        encode(
            Path::new(&std::env::current_dir().unwrap())
                .join("./test_cases/with_import_map")
                .to_str()
                .unwrap()
        )
    );
    let opts = WorkerContextInitOpts {
        service_path: "./test_cases/with_import_map".into(),
        no_module_cache: false,
        import_map_path: Some(inline_import_map),
        env_vars: HashMap::new(),
        events_rx: None,
        timing: None,
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

    let (_conn_tx, conn_rx) = integration_test_helper::create_conn_watch();
    let msg = WorkerRequestMsg {
        req,
        res_tx,
        conn_watch: Some(conn_rx),
    };

    let _ = worker_req_tx.send(msg);

    let res = res_rx.await.unwrap().unwrap();
    assert!(res.status().as_u16() == 200);

    let body_bytes = hyper::body::to_bytes(res.into_body()).await.unwrap();

    assert_eq!(body_bytes, r#"{"message":"ok"}"#);
}
