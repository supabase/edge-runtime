use hyper::{Body, Request, Response};
use std::collections::HashMap;
use std::path::Path;
use tokio::sync::oneshot;
use urlencoding::encode;

use base::worker_ctx::{create_worker, WorkerRequestMsg};
use sb_worker_context::essentials::{EdgeContextInitOpts, EdgeContextOpts, EdgeUserRuntimeOpts};

#[tokio::test]
async fn test_import_map_file_path() {
    let user_rt_opts = EdgeUserRuntimeOpts::default();
    let opts = EdgeContextInitOpts {
        service_path: "./test_cases/with_import_map".into(),
        no_module_cache: false,
        import_map_path: Some("./test_cases/with_import_map/import_map.json".to_string()),
        env_vars: HashMap::new(),
        conf: EdgeContextOpts::UserWorker(user_rt_opts),
    };
    let worker_req_tx = create_worker(opts, None).await.unwrap();
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

    assert_eq!(body_bytes, r#"{"message":"ok"}"#);
}

#[tokio::test]
async fn test_import_map_inline() {
    let user_rt_opts = EdgeUserRuntimeOpts::default();
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
    let opts = EdgeContextInitOpts {
        service_path: "./test_cases/with_import_map".into(),
        no_module_cache: false,
        import_map_path: Some(inline_import_map),
        env_vars: HashMap::new(),
        conf: EdgeContextOpts::UserWorker(user_rt_opts),
    };
    let worker_req_tx = create_worker(opts, None).await.unwrap();
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

    assert_eq!(body_bytes, r#"{"message":"ok"}"#);
}
