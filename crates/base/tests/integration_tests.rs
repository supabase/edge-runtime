#[path = "../src/utils/integration_test_helper.rs"]
mod integration_test_helper;

use std::{
    borrow::Cow,
    collections::HashMap,
    io::{self, Cursor},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
    sync::Arc,
    time::Duration,
};

use anyhow::Context;
use async_tungstenite::WebSocketStream;
use base::{
    integration_test, integration_test_listen_fut, integration_test_with_server_flag,
    rt_worker::worker_ctx::{create_user_worker_pool, create_worker, TerminationToken},
    server::{ServerEvent, ServerFlags, ServerHealth, Tls},
    DecoratorType,
};
use deno_core::serde_json;
use futures_util::{future::BoxFuture, Future, FutureExt, SinkExt, StreamExt};
use http::{Method, Request, Response as HttpResponse, StatusCode};
use http_utils::utils::get_upgrade_type;
use hyper::{body::to_bytes, Body};
use reqwest::{
    header,
    multipart::{Form, Part},
    Response,
};
use reqwest::{Certificate, Client, RequestBuilder};
use sb_core::SharedMetricSource;
use sb_workers::context::{
    MainWorkerRuntimeOpts, WorkerContextInitOpts, WorkerRequestMsg, WorkerRuntimeOpts,
};
use serde::Deserialize;
use serial_test::serial;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    join,
    net::TcpStream,
    sync::{mpsc, oneshot},
    time::{sleep, timeout},
};
use tokio_rustls::{
    rustls::{pki_types::ServerName, ClientConfig, RootCertStore},
    TlsConnector,
};
use tokio_util::{compat::TokioAsyncReadCompatExt, sync::CancellationToken};
use tungstenite::Message;
use urlencoding::encode;

use crate::integration_test_helper::{
    create_test_user_worker, test_user_runtime_opts, test_user_worker_pool_policy, TestBedBuilder,
};

const MB: usize = 1024 * 1024;
const NON_SECURE_PORT: u16 = 8498;
const SECURE_PORT: u16 = 4433;
const TESTBED_DEADLINE_SEC: u64 = 20;

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
        (|resp| async {
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
        (|resp| async {
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
        (|resp| async {
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
        (|resp| async {
            assert!(resp.unwrap().text().await.unwrap().starts_with("meow: "));
        }),
        TerminationToken::new()
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[serial]
async fn test_not_trigger_pku_sigsegv_due_to_jit_compilation_non_cli() {
    let pool_termination_token = TerminationToken::new();
    let main_termination_token = TerminationToken::new();

    // create a user worker pool
    let (_, worker_pool_tx) = create_user_worker_pool(
        integration_test_helper::test_user_worker_pool_policy(),
        None,
        Some(pool_termination_token.clone()),
        vec![],
        None,
        None,
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
        maybe_decorator: None,
        maybe_module_code: None,
        conf: WorkerRuntimeOpts::MainWorker(MainWorkerRuntimeOpts {
            worker_pool_tx,
            shared_metric_src: None,
            event_worker_metric_src: None,
        }),
        static_patterns: vec![],
        maybe_jsx_import_source_config: None,
    };

    let ctx = create_worker((opts, main_termination_token.clone()), None, None)
        .await
        .unwrap();

    let (res_tx, res_rx) = oneshot::channel::<Result<HttpResponse<Body>, hyper::Error>>();

    let req = Request::builder()
        .uri("/slow_resp")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let conn_token = CancellationToken::new();
    let msg = WorkerRequestMsg {
        req,
        res_tx,
        conn_token: Some(conn_token.clone()),
    };

    let _ = ctx.msg_tx.send(msg);

    let res = res_rx.await.unwrap().unwrap();
    assert!(res.status().as_u16() == 200);

    let body_bytes = hyper::body::to_bytes(res.into_body()).await.unwrap();

    assert!(body_bytes.starts_with(b"meow: "));

    conn_token.cancel();
    pool_termination_token.cancel_and_wait().await;
    main_termination_token.cancel_and_wait().await;
}

#[tokio::test]
#[serial]
async fn test_main_worker_options_request() {
    let client = Client::new();
    let req = client
        .request(
            Method::OPTIONS,
            format!("http://localhost:{}/std_user_worker", NON_SECURE_PORT),
        )
        .body(Body::empty())
        .build()
        .unwrap();

    let original = RequestBuilder::from_parts(client, req);

    let request_builder = Some(original);

    integration_test!(
        "./test_cases/main",
        NON_SECURE_PORT,
        "",
        None,
        None,
        request_builder,
        None,
        (|resp| async {
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

    let client = Client::new();
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

    let original = RequestBuilder::from_parts(client, req);

    let request_builder = Some(original);

    integration_test!(
        "./test_cases/main",
        NON_SECURE_PORT,
        "",
        None,
        None,
        request_builder,
        None,
        (|resp| async {
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
        None,
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
        maybe_decorator: None,
        maybe_module_code: None,
        conf: WorkerRuntimeOpts::MainWorker(MainWorkerRuntimeOpts {
            worker_pool_tx,
            shared_metric_src: None,
            event_worker_metric_src: None,
        }),
        static_patterns: vec![],
        maybe_jsx_import_source_config: None,
    };

    let result = create_worker((opts, main_termination_token.clone()), None, None).await;

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

    let client = Client::new();
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

    let original = RequestBuilder::from_parts(client, req);

    let request_builder = Some(original);

    integration_test!(
        "./test_cases/main_with_abort",
        NON_SECURE_PORT,
        "",
        None,
        None,
        request_builder,
        None,
        (|resp| async {
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
    let jsx_tests: Vec<&str> = vec!["./test_cases/jsx", "./test_cases/jsx-2"];
    for test_path in jsx_tests {
        integration_test!(
            test_path,
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
}

#[tokio::test]
async fn test_main_worker_user_worker_mod_evaluate_exception() {
    let pool_termination_token = TerminationToken::new();
    let main_termination_token = TerminationToken::new();

    // create a user worker pool
    let (_, worker_pool_tx) = create_user_worker_pool(
        test_user_worker_pool_policy(),
        None,
        Some(pool_termination_token.clone()),
        vec![],
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let opts = WorkerContextInitOpts {
        service_path: "./test_cases/main".into(),
        no_module_cache: false,
        import_map_path: None,
        env_vars: HashMap::new(),
        events_rx: None,
        timing: None,
        maybe_eszip: None,
        maybe_entrypoint: None,
        maybe_decorator: None,
        maybe_module_code: None,
        conf: WorkerRuntimeOpts::MainWorker(MainWorkerRuntimeOpts {
            worker_pool_tx,
            shared_metric_src: None,
            event_worker_metric_src: None,
        }),
        static_patterns: vec![],
        maybe_jsx_import_source_config: None,
    };

    let ctx = create_worker((opts, main_termination_token.clone()), None, None)
        .await
        .unwrap();

    let (res_tx, res_rx) = oneshot::channel::<Result<HttpResponse<Body>, hyper::Error>>();

    let req = Request::builder()
        .uri("/boot_err_user_worker")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let conn_token = CancellationToken::new();
    let msg = WorkerRequestMsg {
        req,
        res_tx,
        conn_token: Some(conn_token.clone()),
    };

    let _ = ctx.msg_tx.send(msg);

    let res = res_rx.await.unwrap().unwrap();
    assert!(res.status().as_u16() == 500);

    let body_bytes = to_bytes(res.into_body()).await.unwrap();

    assert!(body_bytes.starts_with(b"{\"msg\":\"InvalidWorkerResponse"));
}

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

    let original = RequestBuilder::from_parts(client, req);
    let request_builder = Some(original);

    integration_test!(
        "./test_cases/main",
        NON_SECURE_PORT,
        "",
        None,
        None,
        request_builder,
        maybe_tls,
        (|resp| async {
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
        (|resp| async {
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
    let client = Client::new();
    let req = client
        .request(
            Method::POST,
            format!("http://localhost:{}", NON_SECURE_PORT),
        )
        .body(Body::empty())
        .build()
        .unwrap();

    let original = RequestBuilder::from_parts(client, req);

    let request_builder = Some(original);

    integration_test!(
        "./test_cases/empty-response",
        NON_SECURE_PORT,
        "",
        None,
        None,
        request_builder,
        None,
        (|resp| async {
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
        (|resp| async {
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
    let body_chunk = concat!(
        "--TEST\r\n",
        "Content-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\n",
        "Content-Type: text/plain\r\n",
        "\r\n",
        "testuser\r\n",
        "--TEST--\r\n"
    );

    let content_length = &body_chunk.len();
    let chunks: Vec<Result<_, std::io::Error>> = vec![Ok(body_chunk)];
    let stream = futures_util::stream::iter(chunks);
    let body = Body::wrap_stream(stream);

    let client = Client::new();
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

    let original = RequestBuilder::from_parts(client, req);
    let request_builder = Some(original);

    integration_test!(
        "./test_cases/oak-v12-file-upload",
        NON_SECURE_PORT,
        "",
        None,
        None,
        request_builder,
        None,
        (|resp| async {
            let res = resp.unwrap();
            assert_eq!(res.status().as_u16(), 201);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(body_bytes, "file-type: text/plain");
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_file_upload_real_multipart_bytes() {
    test_oak_file_upload(
        Cow::Borrowed("./test_cases/main"),
        (9.98 * MB as f32) as usize, // < 10MB (in binary)
        |resp| async {
            let res = resp.unwrap();

            assert_eq!(res.status().as_u16(), 201);

            let res = res.text().await;

            assert!(res.is_ok());
            assert_eq!(res.unwrap(), "Success!");
        },
    )
    .await;
}

#[tokio::test]
#[serial]
async fn test_file_upload_size_exceed() {
    test_oak_file_upload(Cow::Borrowed("./test_cases/main"), 10 * MB, |resp| async {
        let res = resp.unwrap();

        assert_eq!(res.status().as_u16(), 500);

        let res = res.text().await;

        assert!(res.is_ok());
        assert_eq!(res.unwrap(), "Error!");
    })
    .await;
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
        (|resp| async {
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
        (|resp| async {
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
        (|resp| async {
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
        (|resp| async {
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
        maybe_decorator: None,
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
async fn req_failure_case_cpu_time_exhausted_2() {
    let tb = TestBedBuilder::new("./test_cases/main_small_cpu_time")
        .with_oneshot_policy(100000)
        .build()
        .await;

    let mut res = tb
        .request(|| {
            Request::builder()
                .uri("/cpu-sync")
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

    assert!(
        buf == "{\"msg\":\"InvalidWorkerResponse: user worker failed to respond\"}"
            || buf
                == "{\"msg\":\"WorkerRequestCancelled: request has been cancelled by supervisor\"}"
    );

    tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
}

#[tokio::test]
#[serial]
async fn req_failture_case_memory_limit_1() {
    let tb = TestBedBuilder::new("./test_cases/main")
        .with_oneshot_policy(100000)
        .build()
        .await;

    let mut res = tb
        .request(|| {
            Request::builder()
                .uri("/array-alloc-sync")
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
async fn req_failture_case_memory_limit_2() {
    let tb = TestBedBuilder::new("./test_cases/main")
        .with_oneshot_policy(100000)
        .build()
        .await;

    let mut res = tb
        .request(|| {
            Request::builder()
                .uri("/array-alloc")
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
    // TODO(Nyannyacha): This test seems a little flaky. If running the entire test
    // dozens of times on the local machine, it will fail with a timeout.

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
            |(_, url, _, mut ev, ..)| async move {
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
            |_| async {}
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

#[tokio::test]
#[serial]
async fn req_failure_case_op_cancel_from_server_due_to_cpu_resource_limit() {
    test_oak_file_upload(
        Cow::Borrowed("./test_cases/main_small_cpu_time"),
        48 * MB,
        |resp| async {
            let res = resp.unwrap();

            assert_eq!(res.status().as_u16(), 500);

            let res = res.json::<ErrorResponsePayload>().await;

            assert!(res.is_ok());
            assert_eq!(
                res.unwrap().msg,
                "WorkerRequestCancelled: request has been cancelled by supervisor"
            );
        },
    )
    .await;
}

async fn test_oak_file_upload<F, R>(main_service: Cow<'static, str>, bytes: usize, resp_callback: F)
where
    F: FnOnce(Result<Response, reqwest::Error>) -> R,
    R: Future<Output = ()>,
{
    let client = Client::builder().build().unwrap();
    let req = client
        .request(
            Method::POST,
            format!("http://localhost:{}/oak-file-upload", NON_SECURE_PORT),
        )
        .multipart(
            Form::new().part(
                "meow",
                Part::bytes(vec![0u8; bytes])
                    .file_name("meow.bin")
                    .mime_str("application/octet-stream")
                    .unwrap(),
            ),
        )
        .build()
        .unwrap();

    let original = RequestBuilder::from_parts(client, req);
    let request_builder = Some(original);

    integration_test!(
        main_service,
        NON_SECURE_PORT,
        "",
        None,
        None,
        request_builder,
        None,
        (|resp| async {
            resp_callback(resp).await;
        }),
        TerminationToken::new()
    );
}

async fn test_websocket_upgrade(maybe_tls: Option<Tls>, use_node_ws: bool) {
    let nonce = tungstenite::handshake::client::generate_key();
    let client = maybe_tls.client();
    let req = client
        .request(
            Method::GET,
            format!(
                "{}://localhost:{}/websocket-upgrade{}",
                maybe_tls.schema(),
                maybe_tls.port(),
                if use_node_ws { "-node" } else { "" }
            ),
        )
        .header(header::CONNECTION, "upgrade")
        .header(header::UPGRADE, "websocket")
        .header(header::SEC_WEBSOCKET_KEY, &nonce)
        .header(header::SEC_WEBSOCKET_VERSION, "13")
        .build()
        .unwrap();

    let original = RequestBuilder::from_parts(client, req);
    let request_builder = Some(original);

    integration_test!(
        "./test_cases/main",
        NON_SECURE_PORT,
        "",
        None,
        None,
        request_builder,
        maybe_tls,
        (|resp| async {
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
async fn test_graceful_shutdown() {
    let token = TerminationToken::new();

    let (server_ev_tx, mut server_ev_rx) = mpsc::unbounded_channel();
    let (metric_tx, metric_rx) = oneshot::channel::<SharedMetricSource>();
    let (tx, rx) = oneshot::channel::<()>();

    tokio::spawn({
        let token = token.clone();
        async move {
            let metric_src = metric_rx.await.unwrap();

            while metric_src.active_io() == 0 {
                tokio::task::yield_now().await;
            }

            assert_eq!(metric_src.active_io(), 1);
            token.cancel();

            tokio::select! {
                Some(ServerEvent::Draining) = server_ev_rx.recv() => {
                    assert_eq!(metric_src.handled_requests(), 0);
                }

                else => {
                    panic!("event sequence does not match != ServerEvent::Draining");
                }
            }

            while metric_src.active_io() > 0 {
                tokio::task::yield_now().await;
            }

            if timeout(Duration::from_secs(10), token.cancel_and_wait())
                .await
                .is_err()
            {
                panic!("failed to terminate server within 10 seconds");
            }

            assert_eq!(metric_src.active_io(), 0);
            assert_eq!(metric_src.handled_requests(), 1);
            assert_eq!(
                metric_src.received_requests(),
                metric_src.handled_requests()
            );

            tx.send(()).unwrap();
        }
    });

    integration_test!(
        "./test_cases/main",
        NON_SECURE_PORT,
        "slow_resp",
        None,
        None,
        None,
        None,
        (
            |(.., mut ev, metric_src)| async move {
                metric_tx.send(metric_src).unwrap();
                tokio::spawn(async move {
                    while let Some(ev) = ev.recv().await {
                        let _ = server_ev_tx.send(ev);
                    }
                });

                None
            },
            |resp| async {
                assert_eq!(resp.unwrap().status().as_u16(), 200);
            }
        ),
        #[manual]
        token
    );

    if timeout(Duration::from_secs(10), rx).await.is_err() {
        panic!("failed to check within 10 seconds");
    }
}

#[tokio::test]
#[serial]
async fn test_websocket_upgrade_deno_non_secure() {
    test_websocket_upgrade(new_localhost_tls(false), false).await;
}

#[tokio::test]
#[serial]
async fn test_websocket_upgrade_deno_secure() {
    test_websocket_upgrade(new_localhost_tls(true), false).await;
}

#[tokio::test]
#[serial]
async fn test_websocket_upgrade_node_non_secure() {
    test_websocket_upgrade(new_localhost_tls(false), true).await;
}

#[tokio::test]
#[serial]
async fn test_websocket_upgrade_node_secure() {
    test_websocket_upgrade(new_localhost_tls(true), true).await;
}

async fn test_decorators(ty: Option<DecoratorType>) {
    let is_disabled = ty.is_none();
    let client = Client::new();

    let endpoint = if is_disabled {
        "tc39".to_string()
    } else {
        serde_json::to_string(&ty).unwrap().replace('\"', "")
    };

    let payload = if is_disabled {
        serde_json::json!({})
    } else {
        serde_json::json!({
            "decoratorType": ty
        })
    };

    let req = client
        .request(
            Method::OPTIONS,
            format!(
                "http://localhost:{}/decorator_{}",
                NON_SECURE_PORT, endpoint
            ),
        )
        .json(&payload)
        .build()
        .unwrap();

    integration_test!(
        "./test_cases/main_with_decorator",
        NON_SECURE_PORT,
        "",
        None,
        None,
        Some(RequestBuilder::from_parts(client, req)),
        None,
        (|resp| async {
            let resp = resp.unwrap();

            if is_disabled {
                assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
                assert!(resp
                    .text()
                    .await
                    .unwrap()
                    .starts_with("{\"msg\":\"InvalidWorkerCreation: worker boot error Uncaught SyntaxError: Invalid or unexpected token"),);
            } else {
                assert_eq!(resp.status(), StatusCode::OK);
                assert_eq!(resp.text().await.unwrap().as_str(), "meow?");
            }
        }),
        TerminationToken::new()
    );
}

#[derive(Deserialize)]
struct ErrorResponsePayload {
    msg: String,
}

#[tokio::test]
#[serial]
async fn test_decorator_should_be_syntax_error() {
    test_decorators(None).await;
}

#[tokio::test]
#[serial]
async fn test_decorator_parse_tc39() {
    test_decorators(Some(DecoratorType::Tc39)).await;
}

#[tokio::test]
#[serial]
async fn test_decorator_parse_typescript_experimental_with_metadata() {
    test_decorators(Some(DecoratorType::TypescriptWithMetadata)).await;
}

#[tokio::test]
#[serial]
async fn send_partial_payload_into_closed_pipe_should_not_be_affected_worker_stability() {
    let tb = TestBedBuilder::new("./test_cases/main")
        .with_oneshot_policy(100000)
        .build()
        .await;

    let mut resp1 = tb
        .request(|| {
            Request::builder()
                .uri("/chunked-char-1000ms")
                .method("GET")
                .body(Body::empty())
                .context("can't make request")
        })
        .await
        .unwrap();

    assert_eq!(resp1.status().as_u16(), StatusCode::OK);

    let resp1_body = resp1.body_mut();
    let resp1_chunk1 = resp1_body.next().await.unwrap().unwrap();

    assert_eq!(resp1_chunk1, "m");

    drop(resp1);
    sleep(Duration::from_secs(1)).await;

    // NOTE(Nyannyacha): Before dc057b0, the statement below panics with the
    // reason `connection closed before message completed`. This is the result
    // of `Deno.serve` failing to properly handle an exception from a previous
    // request.
    let resp2 = tb
        .request(|| {
            Request::builder()
                .uri("/empty-response")
                .method("GET")
                .body(Body::empty())
                .context("can't make request")
        })
        .await
        .unwrap();

    assert_eq!(resp2.status().as_u16(), StatusCode::NO_CONTENT);

    tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
}

#[tokio::test]
#[serial]
async fn oak_with_jsr_specifier() {
    integration_test!(
        "./test_cases/main",
        NON_SECURE_PORT,
        "oak-with-jsr",
        None,
        None,
        None,
        None,
        (|resp| async {
            assert_eq!(resp.unwrap().text().await.unwrap(), "meow");
        }),
        TerminationToken::new()
    );
}

async fn test_slowloris<F, R>(request_read_timeout_ms: u64, maybe_tls: Option<Tls>, test_fn: F)
where
    F: (FnOnce(Box<dyn AsyncReadWrite>) -> R) + Send + 'static,
    R: Future<Output = bool> + Send,
{
    let token = TerminationToken::new();

    let (health_tx, mut health_rx) = mpsc::channel(1);
    let (tx, rx) = oneshot::channel();

    let mut listen_fut = integration_test_listen_fut!(
        NON_SECURE_PORT,
        maybe_tls,
        "./test_cases/main",
        None,
        None,
        ServerFlags {
            request_read_timeout_ms: Some(request_read_timeout_ms),
            ..Default::default()
        },
        health_tx,
        Some(token.clone())
    );

    let req_fut = {
        let token = token.clone();
        async move {
            assert!(test_fn(maybe_tls.stream().await).await);

            if timeout(Duration::from_secs(10), token.cancel_and_wait())
                .await
                .is_err()
            {
                panic!("failed to terminate server within 10 seconds");
            }

            tx.send(()).unwrap();
        }
    };

    let join_fut = tokio::spawn(async move {
        loop {
            if let Some(ServerHealth::Listening(..)) = health_rx.recv().await {
                break;
            }
        }

        req_fut.await;
    });

    tokio::select! {
        _ = join_fut => {}
        _ = &mut listen_fut => {}
    };

    if timeout(Duration::from_secs(10), rx).await.is_err() {
        panic!("failed to check within 10 seconds");
    }
}

async fn test_slowloris_no_prompt_timeout(maybe_tls: Option<Tls>, invert: bool) {
    test_slowloris(
        if invert { u64::MAX } else { 5000 },
        maybe_tls,
        move |mut io| async move {
            static HEADER: &[u8] = b"GET /oak-with-jsr HTTP/1.1\r\nHost: localhost\r\n\r\n";

            let check_io_kind_fn = move |err: std::io::Error| {
                if invert {
                    return true;
                }

                matches!(
                    err.kind(),
                    io::ErrorKind::BrokenPipe | io::ErrorKind::UnexpectedEof
                )
            };

            // > 5000ms
            sleep(Duration::from_secs(10)).await;

            if let Err(err) = io.write_all(HEADER).await {
                return check_io_kind_fn(err);
            }

            if let Err(err) = io.flush().await {
                return check_io_kind_fn(err);
            }

            let mut buf = vec![0; 1_048_576];

            match io.read(&mut buf).await {
                Ok(nread) => {
                    if invert {
                        nread > 0
                    } else {
                        nread == 0
                    }
                }

                Err(err) => check_io_kind_fn(err),
            }
        },
    )
    .await;
}

#[tokio::test]
#[serial]
async fn test_slowloris_no_prompt_timeout_non_secure() {
    test_slowloris_no_prompt_timeout(new_localhost_tls(false), false).await;
}

#[tokio::test]
#[serial]
#[ignore = "too slow"]
async fn test_slowloris_no_prompt_timeout_non_secure_inverted() {
    test_slowloris_no_prompt_timeout(new_localhost_tls(false), true).await;
}

#[tokio::test]
#[serial]
async fn test_slowloris_no_prompt_timeout_secure() {
    test_slowloris_no_prompt_timeout(new_localhost_tls(true), false).await;
}

#[tokio::test]
#[serial]
#[ignore = "too slow"]
async fn test_slowloris_no_prompt_timeout_secure_inverted() {
    test_slowloris_no_prompt_timeout(new_localhost_tls(true), true).await;
}

async fn test_slowloris_slow_header_timedout(maybe_tls: Option<Tls>, invert: bool) {
    test_slowloris(
        if invert { u64::MAX } else { 5000 },
        maybe_tls,
        move |mut io| async move {
            static HEADER: &[u8] = b"GET /oak-with-jsr HTTP/1.1\r\nHost: localhost\r\n\r\n";

            let check_io_kind_fn = move |err: std::io::Error| {
                if invert {
                    return true;
                }

                matches!(
                    err.kind(),
                    io::ErrorKind::BrokenPipe | io::ErrorKind::UnexpectedEof
                )
            };

            // takes 1000ms per each character (ie. > 5000ms)
            for &b in HEADER {
                if let Err(err) = io.write(&[b]).await {
                    return check_io_kind_fn(err);
                }

                if let Err(err) = io.flush().await {
                    return check_io_kind_fn(err);
                }

                sleep(Duration::from_secs(1)).await;
            }

            let mut buf = vec![0; 1_048_576];

            match io.read(&mut buf).await {
                Ok(nread) => {
                    if invert {
                        nread > 0
                    } else {
                        nread == 0
                    }
                }

                Err(err) => check_io_kind_fn(err),
            }
        },
    )
    .await;
}

#[tokio::test]
#[serial]
async fn test_slowloris_slow_header_timedout_non_secure() {
    test_slowloris_slow_header_timedout(new_localhost_tls(false), false).await;
}

#[tokio::test]
#[serial]
#[ignore = "too slow 2x"]
async fn test_slowloris_slow_header_timedout_non_secure_inverted() {
    test_slowloris_slow_header_timedout(new_localhost_tls(false), true).await;
}

#[tokio::test]
#[serial]
async fn test_slowloris_slow_header_timedout_secure() {
    test_slowloris_slow_header_timedout(new_localhost_tls(true), false).await;
}

#[tokio::test]
#[serial]
#[ignore = "too slow 2x"]
async fn test_slowloris_slow_header_timedout_secure_inverted() {
    test_slowloris_slow_header_timedout(new_localhost_tls(true), true).await;
}

async fn test_request_idle_timeout_no_streamed_response(maybe_tls: Option<Tls>) {
    let client = maybe_tls.client();
    let req = client
        .request(
            Method::GET,
            format!(
                "{}://localhost:{}/sleep-5000ms",
                maybe_tls.schema(),
                maybe_tls.port(),
            ),
        )
        .build()
        .unwrap();

    let original = RequestBuilder::from_parts(client, req);
    let request_builder = Some(original);

    integration_test_with_server_flag!(
        ServerFlags {
            request_idle_timeout_ms: Some(1000),
            ..Default::default()
        },
        "./test_cases/main",
        NON_SECURE_PORT,
        "",
        None,
        None,
        request_builder,
        maybe_tls,
        (|resp| async {
            assert_eq!(resp.unwrap().status().as_u16(), StatusCode::GATEWAY_TIMEOUT);
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_request_idle_timeout_no_streamed_response_non_secure() {
    test_request_idle_timeout_no_streamed_response(new_localhost_tls(false)).await;
}

#[tokio::test]
#[serial]
async fn test_request_idle_timeout_no_streamed_response_secure() {
    test_request_idle_timeout_no_streamed_response(new_localhost_tls(true)).await;
}

async fn test_request_idle_timeout_streamed_response(maybe_tls: Option<Tls>) {
    let client = maybe_tls.client();
    let req = client
        .request(
            Method::GET,
            format!(
                "{}://localhost:{}/chunked-char-variable-delay-max-6000ms",
                maybe_tls.schema(),
                maybe_tls.port(),
            ),
        )
        .build()
        .unwrap();

    let original = RequestBuilder::from_parts(client, req);
    let request_builder = Some(original);

    integration_test_with_server_flag!(
        ServerFlags {
            request_idle_timeout_ms: Some(2000),
            ..Default::default()
        },
        "./test_cases/main",
        NON_SECURE_PORT,
        "",
        None,
        None,
        request_builder,
        maybe_tls,
        (|resp| async {
            let resp = resp.unwrap();

            assert_eq!(resp.status().as_u16(), StatusCode::OK);
            assert!(resp.content_length().is_none());

            let mut buf = Vec::<u8>::new();
            let mut bytes_stream = resp.bytes_stream();

            loop {
                match bytes_stream.next().await {
                    Some(Ok(v)) => {
                        buf.extend(v);
                    }

                    Some(Err(_)) => {
                        break;
                    }

                    None => {
                        break;
                    }
                }
            }

            assert_eq!(&buf, b"meo");
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_request_idle_timeout_streamed_response_non_secure() {
    test_request_idle_timeout_streamed_response(new_localhost_tls(false)).await;
}

#[tokio::test]
#[serial]
async fn test_request_idle_timeout_streamed_response_secure() {
    test_request_idle_timeout_streamed_response(new_localhost_tls(true)).await;
}

async fn test_request_idle_timeout_streamed_response_first_chunk_timeout(maybe_tls: Option<Tls>) {
    let client = maybe_tls.client();
    let req = client
        .request(
            Method::GET,
            format!(
                "{}://localhost:{}/chunked-char-first-6000ms",
                maybe_tls.schema(),
                maybe_tls.port(),
            ),
        )
        .build()
        .unwrap();

    let original = RequestBuilder::from_parts(client, req);
    let request_builder = Some(original);

    integration_test_with_server_flag!(
        ServerFlags {
            request_idle_timeout_ms: Some(1000),
            ..Default::default()
        },
        "./test_cases/main",
        NON_SECURE_PORT,
        "",
        None,
        None,
        request_builder,
        maybe_tls,
        (|resp| async {
            let resp = resp.unwrap();

            assert_eq!(resp.status().as_u16(), StatusCode::OK);
            assert!(resp.content_length().is_none());

            let mut buf = Vec::<u8>::new();
            let mut bytes_stream = resp.bytes_stream();

            loop {
                match bytes_stream.next().await {
                    Some(Ok(v)) => {
                        buf.extend(v);
                    }

                    Some(Err(_)) => {
                        break;
                    }

                    None => {
                        break;
                    }
                }
            }

            assert_eq!(&buf, b"");
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_request_idle_timeout_streamed_response_first_chunk_timeout_non_secure() {
    test_request_idle_timeout_streamed_response_first_chunk_timeout(new_localhost_tls(false)).await;
}

#[tokio::test]
#[serial]
async fn test_request_idle_timeout_streamed_response_first_chunk_timeout_secure() {
    test_request_idle_timeout_streamed_response_first_chunk_timeout(new_localhost_tls(true)).await;
}

async fn test_request_idle_timeout_websocket_deno(maybe_tls: Option<Tls>, use_node_ws: bool) {
    let nonce = tungstenite::handshake::client::generate_key();
    let client = maybe_tls.client();
    let req = client
        .request(
            Method::GET,
            format!(
                "{}://localhost:{}/websocket-upgrade-no-send{}",
                maybe_tls.schema(),
                maybe_tls.port(),
                if use_node_ws { "-node" } else { "" }
            ),
        )
        .header(header::CONNECTION, "upgrade")
        .header(header::UPGRADE, "websocket")
        .header(header::SEC_WEBSOCKET_KEY, &nonce)
        .header(header::SEC_WEBSOCKET_VERSION, "13")
        .build()
        .unwrap();

    let original = RequestBuilder::from_parts(client, req);
    let request_builder = Some(original);

    integration_test_with_server_flag!(
        ServerFlags {
            request_idle_timeout_ms: Some(1000),
            ..Default::default()
        },
        "./test_cases/main",
        NON_SECURE_PORT,
        "",
        None,
        None,
        request_builder,
        maybe_tls,
        (|resp| async {
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

            sleep(Duration::from_secs(3)).await;

            ws.send(Message::Text("meow!!".into())).await.unwrap();

            let err = ws.next().await.unwrap().unwrap_err();

            use tungstenite::error::ProtocolError;
            use tungstenite::Error;

            assert!(matches!(
                err,
                Error::Protocol(ProtocolError::ResetWithoutClosingHandshake)
            ));
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_request_idle_timeout_websocket_deno_non_secure() {
    test_request_idle_timeout_websocket_deno(new_localhost_tls(false), false).await;
}

#[tokio::test]
#[serial]
async fn test_request_idle_timeout_websocket_deno_secure() {
    test_request_idle_timeout_websocket_deno(new_localhost_tls(true), false).await;
}

#[tokio::test]
#[serial]
async fn test_request_idle_timeout_websocket_node_non_secure() {
    test_request_idle_timeout_websocket_deno(new_localhost_tls(false), true).await;
}

#[tokio::test]
#[serial]
async fn test_request_idle_timeout_websocket_node_secure() {
    test_request_idle_timeout_websocket_deno(new_localhost_tls(true), true).await;
}

trait AsyncReadWrite: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T> AsyncReadWrite for T where T: AsyncRead + AsyncWrite + Send + Unpin {}

trait TlsExt {
    fn client(&self) -> Client;
    fn schema(&self) -> &'static str;
    fn sock_addr(&self) -> SocketAddr;
    fn port(&self) -> u16;
    fn stream(&self) -> BoxFuture<'static, Box<dyn AsyncReadWrite>>;
}

impl TlsExt for Option<Tls> {
    fn client(&self) -> Client {
        if self.is_some() {
            Client::builder()
                .add_root_certificate(Certificate::from_pem(TLS_LOCALHOST_ROOT_CA).unwrap())
                .build()
                .unwrap()
        } else {
            Client::new()
        }
    }

    fn schema(&self) -> &'static str {
        if self.is_some() {
            "https"
        } else {
            "http"
        }
    }

    fn sock_addr(&self) -> SocketAddr {
        const SOCK_ADDR_SECURE: SocketAddr =
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), SECURE_PORT);

        const SOCK_ADDR_NON_SECURE: SocketAddr =
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), NON_SECURE_PORT);

        if self.is_some() {
            SOCK_ADDR_SECURE
        } else {
            SOCK_ADDR_NON_SECURE
        }
    }

    fn port(&self) -> u16 {
        if self.is_some() {
            SECURE_PORT
        } else {
            NON_SECURE_PORT
        }
    }

    fn stream(&self) -> BoxFuture<'static, Box<dyn AsyncReadWrite>> {
        let use_tls = self.is_some();
        let sock_addr = self.sock_addr();

        async move {
            if use_tls {
                let mut cursor = Cursor::new(Vec::from(TLS_LOCALHOST_ROOT_CA));
                let certs = rustls_pemfile::certs(&mut cursor)
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();

                let mut root_cert_store = RootCertStore::empty();
                let _ = root_cert_store.add_parsable_certificates(certs);

                let config = ClientConfig::builder()
                    .with_root_certificates(root_cert_store)
                    .with_no_client_auth();

                let connector = TlsConnector::from(Arc::new(config));
                let dnsname = ServerName::try_from("localhost").unwrap();

                let stream = TcpStream::connect(sock_addr).await.unwrap();
                let stream = connector.connect(dnsname, stream).await.unwrap();

                Box::new(stream) as Box<dyn AsyncReadWrite>
            } else {
                let stream = TcpStream::connect(sock_addr).await.unwrap();

                Box::new(stream) as Box<dyn AsyncReadWrite>
            }
        }
        .boxed()
    }
}

fn new_localhost_tls(secure: bool) -> Option<Tls> {
    secure.then(|| Tls::new(SECURE_PORT, TLS_LOCALHOST_KEY, TLS_LOCALHOST_CERT).unwrap())
}
