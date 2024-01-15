#[path = "../src/utils/integration_test_helper.rs"]
mod integration_test_helper;

use hyper::{Body, Request, Response};
use sb_workers::context::{WorkerContextInitOpts, WorkerRequestMsg, WorkerRuntimeOpts};
use std::collections::HashMap;
use tokio::sync::oneshot;

use crate::integration_test_helper::{create_test_user_worker, test_user_runtime_opts};

#[tokio::test]
async fn test_node_server() {
    let opts = WorkerContextInitOpts {
        service_path: "./test_cases/node-server".into(),
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
        .uri("/")
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
    assert_eq!(res.status().as_u16(), 200);

    let body_bytes = hyper::body::to_bytes(res.into_body()).await.unwrap();

    assert_eq!(
        body_bytes,
        "Look again at that dot. That's here. That's home. That's us. On it everyone you love, everyone you know, everyone you ever heard of, every human being who ever was, lived out their lives. The aggregate of our joy and suffering, thousands of confident religions, ideologies, and economic doctrines, every hunter and forager, every hero and coward, every creator and destroyer of civilization, every king and peasant, every young couple in love, every mother and father, hopeful child, inventor and explorer, every teacher of morals, every corrupt politician, every 'superstar,' every 'supreme leader,' every saint and sinner in the history of our species lived there-on a mote of dust suspended in a sunbeam."
    );

    req_guard.await;
}
