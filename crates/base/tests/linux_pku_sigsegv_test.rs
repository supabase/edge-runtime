#[cfg(target_os = "linux")]
#[path = "../src/utils/integration_test_helper.rs"]
mod integration_test_helper;

#[cfg(target_os = "linux")]
#[tokio::test]
async fn test_not_trigger_pku_sigsegv_due_to_jit_compilation_non_cli() {
    use std::collections::HashMap;

    use base::rt_worker::worker_ctx::{create_user_worker_pool, create_worker, TerminationToken};
    use http::{Request, Response};
    use hyper::Body;
    use integration_test_helper::create_conn_watch;

    use sb_workers::context::{
        MainWorkerRuntimeOpts, WorkerContextInitOpts, WorkerRequestMsg, WorkerRuntimeOpts,
    };
    use tokio::sync::oneshot;

    let pool_termination_token = TerminationToken::new();
    let main_termination_token = TerminationToken::new();

    // create a user worker pool
    let user_worker_msgs_tx = create_user_worker_pool(
        integration_test_helper::test_user_worker_pool_policy(),
        None,
        Some(pool_termination_token.clone()),
    )
    .await
    .unwrap();

    let opts = WorkerContextInitOpts {
        service_path: "./test_cases/slow_resp".into(),
        no_module_cache: false,
        import_map_path: None,
        env_vars: HashMap::new(),
        events_rx: None,
        timing: None,
        maybe_eszip: None,
        maybe_entrypoint: None,
        maybe_module_code: None,
        conf: WorkerRuntimeOpts::MainWorker(MainWorkerRuntimeOpts {
            worker_pool_tx: user_worker_msgs_tx,
        }),
    };

    let worker_req_tx = create_worker((opts, main_termination_token.clone()))
        .await
        .unwrap();

    let (res_tx, res_rx) = oneshot::channel::<Result<Response<Body>, hyper::Error>>();

    let req = Request::builder()
        .uri("/slow_resp")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let (conn_tx, conn_rx) = create_conn_watch();
    let msg = WorkerRequestMsg {
        req,
        res_tx,
        conn_watch: Some(conn_rx),
    };

    let _ = worker_req_tx.send(msg);

    let res = res_rx.await.unwrap().unwrap();
    assert!(res.status().as_u16() == 200);

    let body_bytes = hyper::body::to_bytes(res.into_body()).await.unwrap();

    assert!(body_bytes.starts_with(b"meow: "));

    drop(conn_tx);
    pool_termination_token.cancel_and_wait().await;
    main_termination_token.cancel_and_wait().await;
}
