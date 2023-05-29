use base::worker_ctx::{create_user_worker_pool, create_worker, WorkerRequestMsg};
use hyper::{Body, Request, Response};
use sb_worker_context::essentials::{EdgeContextInitOpts, EdgeContextOpts, EdgeMainRuntimeOpts};
use std::collections::HashMap;
use tokio::sync::oneshot;

#[tokio::test]
async fn test_main_worker_options_request() {
    // create a user worker pool
    let user_worker_msgs_tx = create_user_worker_pool().await.unwrap();
    let opts = EdgeContextInitOpts {
        service_path: "./test_cases/main".into(),
        no_module_cache: false,
        import_map_path: None,
        env_vars: HashMap::new(),
        conf: EdgeContextOpts::MainWorker(EdgeMainRuntimeOpts {
            worker_pool_tx: user_worker_msgs_tx,
        }),
    };
    let worker_req_tx = create_worker(opts).await.unwrap();
    let (res_tx, res_rx) = oneshot::channel::<Result<Response<Body>, hyper::Error>>();

    let req = Request::builder()
        .uri("/std_user_worker")
        .method("OPTIONS")
        .body(Body::empty())
        .unwrap();

    let msg = WorkerRequestMsg { req, res_tx };
    let _ = worker_req_tx.send(msg);

    let res = res_rx.await.unwrap().unwrap();
    assert!(res.status().as_u16() == 200);

    assert_eq!(
        res.headers().get("Access-Control-Allow-Origin").unwrap(),
        &"*"
    );
    assert_eq!(
        res.headers().get("Access-Control-Allow-Headers").unwrap(),
        &"authorization, x-client-info, apikey"
    );
}

#[tokio::test]
async fn test_main_worker_post_request() {
    // create a user worker pool
    let user_worker_msgs_tx = create_user_worker_pool().await.unwrap();
    let opts = EdgeContextInitOpts {
        service_path: "./test_cases/main".into(),
        no_module_cache: false,
        import_map_path: None,
        env_vars: HashMap::new(),
        conf: EdgeContextOpts::MainWorker(EdgeMainRuntimeOpts {
            worker_pool_tx: user_worker_msgs_tx,
        }),
    };
    let worker_req_tx = create_worker(opts).await.unwrap();
    let (res_tx, res_rx) = oneshot::channel::<Result<Response<Body>, hyper::Error>>();

    let body_chunk = "{ \"name\": \"bar\"}";

    let content_length = &body_chunk.len();
    let chunks: Vec<Result<_, std::io::Error>> = vec![Ok(body_chunk)];
    let stream = futures_util::stream::iter(chunks);
    let body = Body::wrap_stream(stream);

    let req = Request::builder()
        .uri("/std_user_worker")
        .method("POST")
        .header("Content-Type", "application/json")
        .header("Content-Length", content_length.to_string())
        .body(body)
        .unwrap();

    let msg = WorkerRequestMsg { req, res_tx };
    let _ = worker_req_tx.send(msg);

    let res = res_rx.await.unwrap().unwrap();
    assert!(res.status().as_u16() == 200);

    let body_bytes = hyper::body::to_bytes(res.into_body()).await.unwrap();

    assert_eq!(body_bytes, "{\"message\":\"Hello bar from foo!\"}");
}

//#[tokio::test]
//async fn test_main_worker_boot_error() {
//    // create a user worker pool
//    let user_worker_msgs_tx = create_user_worker_pool().await.unwrap();
//    let opts = EdgeContextInitOpts {
//        service_path: "./test_cases/main".into(),
//        no_module_cache: false,
//        import_map_path: Some("./non-existing-import-map.json".to_string()),
//        env_vars: HashMap::new(),
//        conf: EdgeContextOpts::MainWorker(EdgeMainRuntimeOpts {
//            worker_pool_tx: user_worker_msgs_tx,
//        }),
//    };
//    let result = create_worker(opts).await;
//
//    assert!(result.is_err());
//    assert_eq!(result.unwrap_err().to_string(), "worker boot error");
//}

#[tokio::test]
async fn test_main_worker_user_worker_mod_evaluate_exception() {
    // create a user worker pool
    let user_worker_msgs_tx = create_user_worker_pool().await.unwrap();
    let opts = EdgeContextInitOpts {
        service_path: "./test_cases/main".into(),
        no_module_cache: false,
        import_map_path: None,
        env_vars: HashMap::new(),
        conf: EdgeContextOpts::MainWorker(EdgeMainRuntimeOpts {
            worker_pool_tx: user_worker_msgs_tx,
        }),
    };
    let worker_req_tx = create_worker(opts).await.unwrap();
    let (res_tx, res_rx) = oneshot::channel::<Result<Response<Body>, hyper::Error>>();

    let req = Request::builder()
        .uri("/boot_err_user_worker")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let msg = WorkerRequestMsg { req, res_tx };
    let _ = worker_req_tx.send(msg);

    let res = res_rx.await.unwrap().unwrap();
    assert!(res.status().as_u16() == 500);

    let body_bytes = hyper::body::to_bytes(res.into_body()).await.unwrap();

    assert_eq!(
        body_bytes,
        "{\"msg\":\"InvalidWorkerResponse: user worker not available\"}"
    );
}

//#[tokio::test]
//async fn test_main_worker_post_request_with_transfer_encoding() {
//    // create a user worker pool
//    let user_worker_msgs_tx = create_user_worker_pool().await.unwrap();
//    let opts = EdgeContextInitOpts {
//        service_path: "./test_cases/main".into(),
//        no_module_cache: false,
//        import_map_path: None,
//        env_vars: HashMap::new(),
//        conf: EdgeContextOpts::MainWorker(EdgeMainRuntimeOpts {
//            worker_pool_tx: user_worker_msgs_tx,
//        }),
//    };
//    let worker_req_tx = create_worker(opts).await.unwrap();
//    let (res_tx, res_rx) = oneshot::channel::<Result<Response<Body>, hyper::Error>>();
//
//    let chunks: Vec<Result<_, std::io::Error>> = vec![Ok("{\\"name\\":"), Ok("\\"bar\\"}")];
//    let stream = futures_util::stream::iter(chunks);
//    let body = Body::wrap_stream(stream);
//
//    let req = Request::builder()
//        .uri("/std_user_worker")
//        .method("POST")
//        .header("Transfer-Encoding", "chunked")
//        .body(body)
//        .unwrap();
//
//    let msg = WorkerRequestMsg { req, res_tx };
//    let _ = worker_req_tx.send(msg);
//
//    let res = res_rx.await.unwrap().unwrap();
//    assert!(res.status().as_u16() == 200);
//
//    let body_bytes = hyper::body::to_bytes(res.into_body()).await.unwrap();
//
//    assert_eq!(body_bytes, "{\\"message\\":\\"Hello bar from foo!\\"}");
//}
