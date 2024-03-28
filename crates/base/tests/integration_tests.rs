#[path = "../src/utils/integration_test_helper.rs"]
mod integration_test_helper;

use std::{collections::HashMap, path::Path, time::Duration};

use anyhow::Context;
use async_tungstenite::WebSocketStream;
use base::{
    integration_test,
    rt_worker::worker_ctx::{create_user_worker_pool, create_worker, TerminationToken},
    server::{ServerEvent, Tls},
};
use futures_util::{SinkExt, StreamExt};
use http::{Method, Request, StatusCode};
use http_utils::utils::get_upgrade_type;
use hyper::{body::to_bytes, Body};
use reqwest::Certificate;
use sb_workers::context::{MainWorkerRuntimeOpts, WorkerContextInitOpts, WorkerRuntimeOpts};
use serial_test::serial;
use tokio::{join, sync::mpsc};
use tokio_util::compat::TokioAsyncReadCompatExt;
use tungstenite::Message;
use urlencoding::encode;

use crate::integration_test_helper::{
    create_test_user_worker, test_user_runtime_opts, test_user_worker_pool_policy, TestBedBuilder,
};

const NON_SECURE_PORT: u16 = 8498;
const SECURE_PORT: u16 = 4433;
const TESTBED_DEADLINE_SEC: u64 = 5;

const TLS_LOCALHOST_ROOT_CA: &[u8] = include_bytes!("./fixture/tls/root-ca.pem");
const TLS_LOCALHOST_CERT: &[u8] = include_bytes!("./fixture/tls/localhost.pem");
const TLS_LOCALHOST_KEY: &[u8] = include_bytes!("./fixture/tls/localhost-key.pem");

#[tokio::test]
#[serial]
async fn test_custom_readable_stream_response() {
    integration_test!(
        "./test_cases/main",
        NON_SECURE_PORT,
        "readable-stream-resp",
        None,
        None,
        None,
        None,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            assert_eq!(
                resp.unwrap().text().await.unwrap(),
                "Hello world from streams"
            );
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_import_map_file_path() {
    integration_test!(
        "./test_cases/with_import_map",
        NON_SECURE_PORT,
        "",
        None,
        Some("./test_cases/with_import_map/import_map.json".to_string()),
        None,
        None,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 200);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(body_bytes, r#"{"message":"ok"}"#);
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_import_map_inline() {
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

    integration_test!(
        "./test_cases/with_import_map",
        NON_SECURE_PORT,
        "",
        None,
        Some(inline_import_map),
        None,
        None,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 200);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(body_bytes, r#"{"message":"ok"}"#);
        }),
        TerminationToken::new()
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[serial]
async fn test_not_trigger_pku_sigsegv_due_to_jit_compilation_cli() {
    integration_test!(
        "./test_cases/main",
        NON_SECURE_PORT,
        "slow_resp",
        None,
        None,
        None,
        None,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            assert!(resp.unwrap().text().await.unwrap().starts_with("meow: "));
        }),
        TerminationToken::new()
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[serial]
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
    let (_, worker_pool_tx) = create_user_worker_pool(
        integration_test_helper::test_user_worker_pool_policy(),
        None,
        Some(pool_termination_token.clone()),
        vec![],
        None,
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
            worker_pool_tx,
            shared_metric_src: None,
            event_worker_metric_src: None,
        }),
        static_patterns: vec![],
        maybe_jsx_import_source_config: None,
    };

    let (_, worker_req_tx) = create_worker((opts, main_termination_token.clone()), None)
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

#[tokio::test]
#[serial]
async fn test_main_worker_options_request() {
    let client = reqwest::Client::new();
    let req = client
        .request(
            Method::OPTIONS,
            format!("http://localhost:{}/std_user_worker", NON_SECURE_PORT),
        )
        .body(Body::empty())
        .build()
        .unwrap();

    let original = reqwest::RequestBuilder::from_parts(client, req);

    let request_builder = Some(original);

    integration_test!(
        "./test_cases/main",
        NON_SECURE_PORT,
        "",
        None,
        None,
        request_builder,
        None,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 200);

            assert_eq!(
                res.headers().get("Access-Control-Allow-Origin").unwrap(),
                &"*"
            );
            assert_eq!(
                res.headers().get("Access-Control-Allow-Headers").unwrap(),
                &"authorization, x-client-info, apikey"
            );
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_main_worker_post_request() {
    let body_chunk = "{ \"name\": \"bar\"}";

    let content_length = &body_chunk.len();
    let chunks: Vec<Result<_, std::io::Error>> = vec![Ok(body_chunk)];
    let stream = futures_util::stream::iter(chunks);
    let body = Body::wrap_stream(stream);

    let client = reqwest::Client::new();
    let req = client
        .request(
            Method::POST,
            format!("http://localhost:{}/std_user_worker", NON_SECURE_PORT),
        )
        .body(body)
        .header("Content-Type", "application/json")
        .header("Content-Length", content_length.to_string())
        .build()
        .unwrap();

    let original = reqwest::RequestBuilder::from_parts(client, req);

    let request_builder = Some(original);

    integration_test!(
        "./test_cases/main",
        NON_SECURE_PORT,
        "",
        None,
        None,
        request_builder,
        None,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 200);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(body_bytes, "{\"message\":\"Hello bar from foo!\"}");
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_main_worker_boot_error() {
    let pool_termination_token = TerminationToken::new();
    let main_termination_token = TerminationToken::new();

    // create a user worker pool
    let (_, worker_pool_tx) = create_user_worker_pool(
        test_user_worker_pool_policy(),
        None,
        Some(pool_termination_token.clone()),
        vec![],
        None,
    )
    .await
    .unwrap();

    let opts = WorkerContextInitOpts {
        service_path: "./test_cases/main".into(),
        no_module_cache: false,
        import_map_path: Some("./non-existing-import-map.json".to_string()),
        env_vars: HashMap::new(),
        events_rx: None,
        timing: None,
        maybe_eszip: None,
        maybe_entrypoint: None,
        maybe_module_code: None,
        conf: WorkerRuntimeOpts::MainWorker(MainWorkerRuntimeOpts {
            worker_pool_tx,
            shared_metric_src: None,
            event_worker_metric_src: None,
        }),
        static_patterns: vec![],
        maybe_jsx_import_source_config: None,
    };

    let result = create_worker((opts, main_termination_token.clone()), None).await;

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .starts_with("worker boot error"));

    pool_termination_token.cancel_and_wait().await;
    main_termination_token.cancel_and_wait().await;
}

#[tokio::test]
#[serial]
async fn test_main_worker_abort_request() {
    let body_chunk = "{ \"name\": \"bar\"}";

    let content_length = &body_chunk.len();
    let chunks: Vec<Result<_, std::io::Error>> = vec![Ok(body_chunk)];
    let stream = futures_util::stream::iter(chunks);
    let body = Body::wrap_stream(stream);

    let client = reqwest::Client::new();
    let req = client
        .request(
            Method::POST,
            format!("http://localhost:{}/std_user_worker", NON_SECURE_PORT),
        )
        .body(body)
        .header("Content-Type", "application/json")
        .header("Content-Length", content_length.to_string())
        .build()
        .unwrap();

    let original = reqwest::RequestBuilder::from_parts(client, req);

    let request_builder = Some(original);

    integration_test!(
        "./test_cases/main_with_abort",
        NON_SECURE_PORT,
        "",
        None,
        None,
        request_builder,
        None,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 500);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(
                body_bytes,
                "{\"msg\":\"AbortError: The signal has been aborted\"}"
            );
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_main_worker_with_jsx_function() {
    integration_test!(
        "./test_cases/jsx",
        NON_SECURE_PORT,
        "jsx-server",
        None,
        None,
        None,
        None,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 200);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(
                body_bytes,
                r#"{"type":"div","props":{"children":"Hello"},"__k":null,"__":null,"__b":0,"__e":null,"__c":null,"__v":-1,"__i":-1,"__u":0}"#
            );
        }),
        TerminationToken::new()
    );
}

//#[tokio::test]
//async fn test_main_worker_user_worker_mod_evaluate_exception() {
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
//    let req = Request::builder()
//        .uri("/boot_err_user_worker")
//        .method("GET")
//        .body(Body::empty())
//        .unwrap();
//
//    let msg = WorkerRequestMsg { req, res_tx };
//    let _ = worker_req_tx.send(msg);
//
//    let res = res_rx.await.unwrap().unwrap();
//    assert!(res.status().as_u16() == 500);
//
//    let body_bytes = hyper::body::to_bytes(res.into_body()).await.unwrap();
//
//    assert_eq!(
//        body_bytes,
//        "{\\"msg\\":\\"InvalidWorkerResponse: user worker not available\\"}"
//    );
//}

async fn test_main_worker_post_request_with_transfer_encoding(maybe_tls: Option<Tls>) {
    let chunks: Vec<Result<_, std::io::Error>> = vec![Ok("{\"name\":"), Ok("\"bar\"}")];
    let stream = futures_util::stream::iter(chunks);
    let body = Body::wrap_stream(stream);

    let client = maybe_tls.client();
    let req = client
        .request(
            Method::POST,
            format!(
                "{}://localhost:{}/std_user_worker",
                maybe_tls.schema(),
                maybe_tls.port(),
            ),
        )
        .body(body)
        .header("Transfer-Encoding", "chunked")
        .build()
        .unwrap();

    let original = reqwest::RequestBuilder::from_parts(client, req);
    let request_builder = Some(original);

    integration_test!(
        "./test_cases/main",
        NON_SECURE_PORT,
        "",
        None,
        None,
        request_builder,
        maybe_tls,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 200);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(body_bytes, "{\"message\":\"Hello bar from foo!\"}");
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_main_worker_post_request_with_transfer_encoding_non_secure() {
    test_main_worker_post_request_with_transfer_encoding(new_localhost_tls(false)).await;
}

#[tokio::test]
#[serial]
async fn test_main_worker_post_request_with_transfer_encoding_secure() {
    test_main_worker_post_request_with_transfer_encoding(new_localhost_tls(true)).await;
}

#[tokio::test]
#[serial]
async fn test_null_body_with_204_status() {
    integration_test!(
        "./test_cases/empty-response",
        NON_SECURE_PORT,
        "",
        None,
        None,
        None,
        None,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 204);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(body_bytes.len(), 0);
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_null_body_with_204_status_post() {
    let client = reqwest::Client::new();
    let req = client
        .request(
            Method::POST,
            format!("http://localhost:{}", NON_SECURE_PORT),
        )
        .body(Body::empty())
        .build()
        .unwrap();

    let original = reqwest::RequestBuilder::from_parts(client, req);

    let request_builder = Some(original);

    integration_test!(
        "./test_cases/empty-response",
        NON_SECURE_PORT,
        "",
        None,
        None,
        request_builder,
        None,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 204);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(body_bytes.len(), 0);
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_oak_server() {
    integration_test!(
        "./test_cases/oak",
        NON_SECURE_PORT,
        "oak",
        None,
        None,
        None,
        None,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 200);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(
                body_bytes,
                "This is an example Oak server running on Edge Functions!"
            );
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_file_upload() {
    let body_chunk = "--TEST\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\nContent-Type: text/plain\r\n\r\ntestuser\r\n--TEST--\r\n";

    let content_length = &body_chunk.len();
    let chunks: Vec<Result<_, std::io::Error>> = vec![Ok(body_chunk)];
    let stream = futures_util::stream::iter(chunks);
    let body = Body::wrap_stream(stream);

    let client = reqwest::Client::new();
    let req = client
        .request(
            Method::POST,
            format!("http://localhost:{}/file-upload", NON_SECURE_PORT),
        )
        .header("Content-Type", "multipart/form-data; boundary=TEST")
        .header("Content-Length", content_length.to_string())
        .body(body)
        .build()
        .unwrap();

    let original = reqwest::RequestBuilder::from_parts(client, req);

    let request_builder = Some(original);

    integration_test!(
        "./test_cases/oak",
        NON_SECURE_PORT,
        "",
        None,
        None,
        request_builder,
        None,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 201);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(body_bytes, "file-type: text/plain");
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_node_server() {
    integration_test!(
        "./test_cases/node-server",
        NON_SECURE_PORT,
        "",
        None,
        None,
        None,
        None,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert_eq!(res.status().as_u16(), 200);
            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(
                body_bytes,
                concat!(
                    "Look again at that dot. That's here. That's home. That's us. On it everyone you love, ",
                    "everyone you know, everyone you ever heard of, every human being who ever was, lived out ",
                    "their lives. The aggregate of our joy and suffering, thousands of confident religions, ideologies, ",
                    "and economic doctrines, every hunter and forager, every hero and coward, every creator and destroyer of ",
                    "civilization, every king and peasant, every young couple in love, every mother and father, hopeful child, ",
                    "inventor and explorer, every teacher of morals, every corrupt politician, every 'superstar,' every 'supreme leader,' ",
                    "every saint and sinner in the history of our species lived there-on a mote of dust suspended in a sunbeam."
                )
            );
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_tls_throw_invalid_data() {
    integration_test!(
        "./test_cases/tls_invalid_data",
        NON_SECURE_PORT,
        "",
        None,
        None,
        None,
        None,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 200);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(body_bytes, r#"{"passed":true}"#);
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_user_worker_json_imports() {
    integration_test!(
        "./test_cases/json_import",
        NON_SECURE_PORT,
        "",
        None,
        None,
        None,
        None,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 200);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(body_bytes, r#"{"version":"1.0.0"}"#);
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_user_imports_npm() {
    integration_test!(
        "./test_cases/npm",
        NON_SECURE_PORT,
        "",
        None,
        None,
        None,
        None,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert!(res.status().as_u16() == 200);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(
                body_bytes,
                r#"{"is_even":true,"hello":"","numbers":{"Uno":1,"Dos":2}}"#
            );
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_worker_boot_invalid_imports() {
    let opts = WorkerContextInitOpts {
        service_path: "./test_cases/invalid_imports".into(),
        no_module_cache: false,
        import_map_path: None,
        env_vars: HashMap::new(),
        events_rx: None,
        timing: None,
        maybe_eszip: None,
        maybe_entrypoint: None,
        maybe_module_code: None,
        conf: WorkerRuntimeOpts::UserWorker(test_user_runtime_opts()),
        static_patterns: vec![],
        maybe_jsx_import_source_config: None,
    };

    let result = create_test_user_worker(opts).await;

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .starts_with("worker boot error"));
}

#[tokio::test]
#[serial]
async fn req_failure_case_timeout() {
    let tb = TestBedBuilder::new("./test_cases/main")
        // NOTE: It should be small enough that the worker pool rejects the
        // request.
        .with_oneshot_policy(10)
        .build()
        .await;

    let req_body_fn = || {
        Request::builder()
            .uri("/slow_resp")
            .method("GET")
            .body(Body::empty())
            .context("can't make request")
    };

    let (res1, res2) = join!(tb.request(req_body_fn), tb.request(req_body_fn));

    let res_iter = vec![res1, res2].into_iter();
    let mut found_timeout = false;

    for res in res_iter {
        let mut res = res.unwrap();

        if !found_timeout {
            let buf = to_bytes(res.body_mut()).await.unwrap();
            let status_500 = res.status() == StatusCode::INTERNAL_SERVER_ERROR;
            let valid_output =
                buf == "{\"msg\":\"InvalidWorkerCreation: worker did not respond in time\"}";

            found_timeout = status_500 && valid_output;
        }
    }

    tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
    assert!(found_timeout);
}

#[tokio::test]
#[serial]
async fn req_failure_case_cpu_time_exhausted() {
    let tb = TestBedBuilder::new("./test_cases/main_small_cpu_time")
        .with_oneshot_policy(100000)
        .build()
        .await;

    let mut res = tb
        .request(|| {
            Request::builder()
                .uri("/slow_resp")
                .method("GET")
                .body(Body::empty())
                .context("can't make request")
        })
        .await
        .unwrap();

    let buf = to_bytes(res.body_mut()).await.unwrap();

    assert_eq!(
        buf,
        "{\"msg\":\"WorkerRequestCancelled: request has been cancelled by supervisor\"}"
    );

    tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
}

#[tokio::test]
#[serial]
async fn req_failure_case_wall_clock_reached() {
    let tb = TestBedBuilder::new("./test_cases/main_small_wall_clock")
        .with_oneshot_policy(100000)
        .build()
        .await;

    let mut res = tb
        .request(|| {
            Request::builder()
                .uri("/slow_resp")
                .method("GET")
                .body(Body::empty())
                .context("can't make request")
        })
        .await
        .unwrap();

    let buf = to_bytes(res.body_mut()).await.unwrap();

    assert_eq!(
        buf,
        "{\"msg\":\"WorkerRequestCancelled: request has been cancelled by supervisor\"}"
    );

    tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
}

#[tokio::test]
#[serial]
async fn req_failure_case_wall_clock_reached_less_than_100ms() {
    let tb = TestBedBuilder::new("./test_cases/main_small_wall_clock_less_than_100ms")
        .with_oneshot_policy(100000)
        .build()
        .await;

    let mut res = tb
        .request(|| {
            Request::builder()
                .uri("/slow_resp")
                .method("GET")
                .body(Body::empty())
                .context("can't make request")
        })
        .await
        .unwrap();

    let buf = to_bytes(res.body_mut()).await.unwrap();

    assert!(
        buf == "{\"msg\":\"InvalidWorkerResponse: user worker failed to respond\"}"
            || buf == "{\"msg\":\"InvalidWorkerCreation: worker did not respond in time\"}"
            || buf
                == "{\"msg\":\"WorkerRequestCancelled: request has been cancelled by supervisor\"}"
    );

    tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
}

async fn req_failure_case_intentional_peer_reset(maybe_tls: Option<Tls>) {
    let (server_ev_tx, mut server_ev_rx) = mpsc::unbounded_channel();

    integration_test!(
        "./test_cases/main",
        NON_SECURE_PORT,
        "slow_resp",
        None,
        None,
        None,
        maybe_tls.clone(),
        (
            |_port: u16,
             url: &'static str,
             _req_builder: Option<reqwest::RequestBuilder>,
             mut ev: mpsc::UnboundedReceiver<ServerEvent>| async move {
                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            Some(ev) = ev.recv() => {
                                let _ = server_ev_tx.send(ev);
                                break;
                            }

                            else => continue
                        }
                    }
                });

                Some(
                    maybe_tls
                        .client()
                        .get(format!(
                            "{}://localhost:{}/{}",
                            maybe_tls.schema(),
                            maybe_tls.port(),
                            url
                        ))
                        .timeout(Duration::from_millis(100))
                        .send()
                        .await,
                )
            },
            |_resp: Result<reqwest::Response, reqwest::Error>| async {}
        ),
        TerminationToken::new()
    );

    let ev = loop {
        tokio::select! {
            Some(ev) = server_ev_rx.recv() => break ev,
            else => continue,
        }
    };

    assert!(matches!(ev, ServerEvent::ConnectionError(e) if e.is_incomplete_message()));
}

#[tokio::test]
#[serial]
async fn req_failure_case_intentional_peer_reset_non_secure() {
    req_failure_case_intentional_peer_reset(new_localhost_tls(false)).await;
}

#[tokio::test]
#[serial]
async fn req_failure_case_intentional_peer_reset_secure() {
    req_failure_case_intentional_peer_reset(new_localhost_tls(true)).await;
}

async fn test_websocket_upgrade(maybe_tls: Option<Tls>) {
    let nonce = tungstenite::handshake::client::generate_key();
    let client = maybe_tls.client();
    let req = client
        .request(
            Method::GET,
            format!(
                "{}://localhost:{}/websocket-upgrade",
                maybe_tls.schema(),
                maybe_tls.port(),
            ),
        )
        .header(reqwest::header::CONNECTION, "upgrade")
        .header(reqwest::header::UPGRADE, "websocket")
        .header(reqwest::header::SEC_WEBSOCKET_KEY, &nonce)
        .header(reqwest::header::SEC_WEBSOCKET_VERSION, "13")
        .build()
        .unwrap();

    let original = reqwest::RequestBuilder::from_parts(client, req);
    let request_builder = Some(original);

    integration_test!(
        "./test_cases/main",
        NON_SECURE_PORT,
        "",
        None,
        None,
        request_builder,
        maybe_tls,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            let accepted = get_upgrade_type(res.headers());

            assert!(res.status().as_u16() == 101);
            assert!(accepted.is_some());
            assert_eq!(accepted.as_ref().unwrap(), "websocket");

            let upgraded = res.upgrade().await.unwrap();
            let mut ws = WebSocketStream::from_raw_socket(
                upgraded.compat(),
                tungstenite::protocol::Role::Client,
                None,
            )
            .await;

            assert_eq!(
                ws.next().await.unwrap().unwrap().into_text().unwrap(),
                "meow"
            );

            ws.send(Message::Text("meow!!".into())).await.unwrap();
            assert_eq!(
                ws.next().await.unwrap().unwrap().into_text().unwrap(),
                "meow!!"
            );
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_websocket_upgrade_non_secure() {
    test_websocket_upgrade(new_localhost_tls(false)).await;
}

#[tokio::test]
#[serial]
async fn test_websocket_upgrade_secure() {
    test_websocket_upgrade(new_localhost_tls(true)).await;
}

trait TlsExt {
    fn client(&self) -> reqwest::Client;
    fn schema(&self) -> &'static str;
    fn port(&self) -> u16;
}

impl TlsExt for Option<Tls> {
    fn client(&self) -> reqwest::Client {
        if self.is_some() {
            reqwest::Client::builder()
                .add_root_certificate(Certificate::from_pem(TLS_LOCALHOST_ROOT_CA).unwrap())
                .build()
                .unwrap()
        } else {
            reqwest::Client::new()
        }
    }

    fn schema(&self) -> &'static str {
        if self.is_some() {
            "https"
        } else {
            "http"
        }
    }

    fn port(&self) -> u16 {
        if self.is_some() {
            SECURE_PORT
        } else {
            NON_SECURE_PORT
        }
    }
}

fn new_localhost_tls(secure: bool) -> Option<Tls> {
    secure.then(|| Tls::new(SECURE_PORT, TLS_LOCALHOST_KEY, TLS_LOCALHOST_CERT).unwrap())
}
