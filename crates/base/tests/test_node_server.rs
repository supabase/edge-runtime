use base::rt_worker::worker_ctx::create_worker;
use hyper::{Body, Request, Response};
use sb_worker_context::essentials::{
    UserWorkerRuntimeOpts, WorkerContextInitOpts, WorkerRequestMsg, WorkerRuntimeOpts,
};
use std::collections::HashMap;
use tokio::sync::oneshot;

#[tokio::test]
async fn test_node_server() {
    let user_rt_opts = UserWorkerRuntimeOpts::default();
    let opts = WorkerContextInitOpts {
        service_path: "./test_cases/node-server".into(),
        no_module_cache: false,
        import_map_path: None,
        env_vars: HashMap::new(),
        events_rx: None,
        maybe_eszip: None,
        maybe_entrypoint: None,
        maybe_module_code: None,
        conf: WorkerRuntimeOpts::UserWorker(user_rt_opts),
        watch: None,
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
    assert_eq!(res.status().as_u16(), 200);

    let body_bytes = hyper::body::to_bytes(res.into_body()).await.unwrap();

    assert_eq!(
        body_bytes,
        "Look again at that dot. That's here. That's home. That's us. On it everyone you love, everyone you know, everyone you ever heard of, every human being who ever was, lived out their lives. The aggregate of our joy and suffering, thousands of confident religions, ideologies, and economic doctrines, every hunter and forager, every hero and coward, every creator and destroyer of civilization, every king and peasant, every young couple in love, every mother and father, hopeful child, inventor and explorer, every teacher of morals, every corrupt politician, every 'superstar,' every 'supreme leader,' every saint and sinner in the history of our species lived there-on a mote of dust suspended in a sunbeam."
    );
}
