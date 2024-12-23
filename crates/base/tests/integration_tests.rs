#![allow(clippy::arc_with_non_send_sync)]
#![allow(clippy::async_yields_async)]

use deno_config::JsxImportSourceConfig;
use graph::{emitter::EmitterFactory, generate_binary_eszip, EszipPayloadKind};
use http_v02::{self as http, HeaderValue};
use hyper_v014 as hyper;
use reqwest_v011 as reqwest;
use sb_event_worker::events::{LogLevel, WorkerEvents};
use url::Url;

use std::{
    borrow::Cow,
    collections::HashMap,
    io::{self, BufRead, Cursor},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::Context;
use async_tungstenite::WebSocketStream;
use base::{
    integration_test, integration_test_listen_fut, integration_test_with_server_flag,
    server::{Server, ServerEvent, ServerFlags, ServerHealth, Tls},
    worker, DecoratorType,
};
use base::{
    utils::test_utils::{
        self, create_test_user_worker, test_user_runtime_opts, test_user_worker_pool_policy,
        TestBedBuilder,
    },
    worker::TerminationToken,
};
use deno_core::serde_json::{self, json};
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
    fs,
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
    let (_, worker_pool_tx) = worker::create_user_worker_pool(
        Arc::default(),
        test_utils::test_user_worker_pool_policy(),
        None,
        Some(pool_termination_token.clone()),
        vec![],
        None,
        None,
    )
    .await
    .unwrap();

    let surface = worker::WorkerSurfaceBuilder::new()
        .init_opts(WorkerContextInitOpts {
            service_path: "./test_cases/slow_resp".into(),
            no_module_cache: false,
            import_map_path: None,
            env_vars: HashMap::new(),
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
            maybe_s3_fs_config: None,
            maybe_tmp_fs_config: None,
        })
        .termination_token(main_termination_token.clone())
        .build()
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

    let _ = surface.msg_tx.send(msg);

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
    let (_, worker_pool_tx) = worker::create_user_worker_pool(
        Arc::default(),
        test_user_worker_pool_policy(),
        None,
        Some(pool_termination_token.clone()),
        vec![],
        None,
        None,
    )
    .await
    .unwrap();

    let result = worker::WorkerSurfaceBuilder::new()
        .init_opts(WorkerContextInitOpts {
            service_path: "./test_cases/main".into(),
            no_module_cache: false,
            import_map_path: Some("./non-existing-import-map.json".to_string()),
            env_vars: HashMap::new(),
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
            maybe_s3_fs_config: None,
            maybe_tmp_fs_config: None,
        })
        .termination_token(main_termination_token.clone())
        .build()
        .await;

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
    let (_, worker_pool_tx) = worker::create_user_worker_pool(
        Arc::default(),
        test_user_worker_pool_policy(),
        None,
        Some(pool_termination_token.clone()),
        vec![],
        None,
        None,
    )
    .await
    .unwrap();

    let surface = worker::WorkerSurfaceBuilder::new()
        .init_opts(WorkerContextInitOpts {
            service_path: "./test_cases/main".into(),
            no_module_cache: false,
            import_map_path: None,
            env_vars: HashMap::new(),
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
            maybe_s3_fs_config: None,
            maybe_tmp_fs_config: None,
        })
        .termination_token(main_termination_token.clone())
        .build()
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

    let _ = surface.msg_tx.send(msg);

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
        None,
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
    test_oak_file_upload(
        Cow::Borrowed("./test_cases/main"),
        10 * MB,
        None,
        |resp| async {
            let res = resp.unwrap();

            assert_eq!(res.status().as_u16(), 500);

            let res = res.text().await;

            assert!(res.is_ok());
            assert_eq!(res.unwrap(), "Error!");
        },
    )
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
        timing: None,
        maybe_eszip: None,
        maybe_entrypoint: None,
        maybe_decorator: None,
        maybe_module_code: None,
        conf: WorkerRuntimeOpts::UserWorker(test_user_runtime_opts()),
        static_patterns: vec![],

        maybe_jsx_import_source_config: None,
        maybe_s3_fs_config: None,
        maybe_tmp_fs_config: None,
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
async fn test_worker_boot_with_0_byte_eszip() {
    let opts = WorkerContextInitOpts {
        service_path: "./test_cases/meow".into(),
        no_module_cache: false,
        import_map_path: None,
        env_vars: HashMap::new(),
        timing: None,
        maybe_eszip: Some(EszipPayloadKind::VecKind(vec![])),
        maybe_entrypoint: Some("file:///src/index.ts".to_string()),
        maybe_decorator: None,
        maybe_module_code: None,
        conf: WorkerRuntimeOpts::UserWorker(test_user_runtime_opts()),
        static_patterns: vec![],

        maybe_jsx_import_source_config: None,
        maybe_s3_fs_config: None,
        maybe_tmp_fs_config: None,
    };

    let result = create_test_user_worker(opts).await;

    assert!(result.is_err());
    assert!(format!("{:#}", result.unwrap_err())
        .starts_with("worker boot error: unexpected end of file"));
}

#[tokio::test]
#[serial]
async fn test_worker_boot_with_invalid_entrypoint() {
    let opts = WorkerContextInitOpts {
        service_path: "./test_cases/meow".into(),
        no_module_cache: false,
        import_map_path: None,
        env_vars: HashMap::new(),
        timing: None,
        maybe_eszip: None,
        maybe_entrypoint: Some("file:///meow/mmmmeeeow.ts".to_string()),
        maybe_decorator: None,
        maybe_module_code: None,
        conf: WorkerRuntimeOpts::UserWorker(test_user_runtime_opts()),
        static_patterns: vec![],

        maybe_jsx_import_source_config: None,
        maybe_s3_fs_config: None,
        maybe_tmp_fs_config: None,
    };

    let result = create_test_user_worker(opts).await;

    assert!(result.is_err());
    assert!(
        format!("{:#}", result.unwrap_err()).starts_with("worker boot error: failed to read path")
    );
}

#[tokio::test]
#[serial]
async fn req_failure_case_timeout() {
    let tb = TestBedBuilder::new("./test_cases/main")
        // NOTE: It should be small enough that the worker pool rejects the
        // request.
        .with_oneshot_policy(Some(10))
        .build()
        .await;

    let req_body_fn = |b: http::request::Builder| {
        b.uri("/slow_resp")
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
        .with_oneshot_policy(None)
        .build()
        .await;

    let mut res = tb
        .request(|b| {
            b.uri("/slow_resp")
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
        .with_oneshot_policy(None)
        .build()
        .await;

    let mut res = tb
        .request(|b| {
            b.uri("/cpu-sync")
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
        .with_oneshot_policy(None)
        .build()
        .await;

    let mut res = tb
        .request(|b| {
            b.uri("/slow_resp")
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
        .with_oneshot_policy(None)
        .build()
        .await;

    let mut res = tb
        .request(|b| {
            b.uri("/array-alloc-sync")
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
        .with_oneshot_policy(None)
        .build()
        .await;

    let mut res = tb
        .request(|b| {
            b.uri("/array-alloc")
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
        .with_oneshot_policy(None)
        .build()
        .await;

    let mut res = tb
        .request(|b| {
            b.uri("/slow_resp")
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
        120 * MB,
        None,
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

#[tokio::test]
#[serial]
async fn req_failure_case_op_cancel_from_server_due_to_cpu_resource_limit_2() {
    test_oak_file_upload(
        Cow::Borrowed("./test_cases/main_small_cpu_time"),
        10 * MB,
        Some("image/png"),
        |resp| async {
            let res = resp.unwrap();

            assert_eq!(res.status().as_u16(), 500);

            let res = res.json::<ErrorResponsePayload>().await;

            assert!(res.is_ok());

            let msg = res.unwrap().msg;

            assert!(!msg.starts_with("TypeError: request body receiver not connected"));
            assert_eq!(
                msg,
                "WorkerRequestCancelled: request has been cancelled by supervisor"
            );
        },
    )
    .await;
}

async fn test_oak_file_upload<F, R>(
    main_service: Cow<'static, str>,
    bytes: usize,
    mime: Option<&str>,
    resp_callback: F,
) where
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
                    .mime_str(mime.unwrap_or("application/octet-stream"))
                    .unwrap(),
            ),
        )
        .build()
        .unwrap();

    let original = RequestBuilder::from_parts(client, req);
    let request_builder = Some(original);

    integration_test_with_server_flag!(
        ServerFlags {
            request_buffer_size: Some(1024),
            ..Default::default()
        },
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
                    .starts_with("{\"msg\":\"InvalidWorkerCreation: worker boot error: Uncaught SyntaxError: Invalid or unexpected token"),);
            } else {
                assert_eq!(resp.status(), StatusCode::OK);
                assert_eq!(resp.text().await.unwrap().as_str(), "meow?");
            }
        }),
        TerminationToken::new()
    );
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
        .with_oneshot_policy(None)
        .build()
        .await;

    let mut resp1 = tb
        .request(|b| {
            b.uri("/chunked-char-1000ms")
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
        .request(|b| {
            b.uri("/empty-response")
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
#[ignore = "running too much tests"]
async fn test_request_idle_timeout_no_streamed_response_non_secure_1000() {
    for _ in 0..1000 {
        test_request_idle_timeout_no_streamed_response(new_localhost_tls(false)).await;
    }
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

#[tokio::test]
#[serial]
async fn test_should_not_hang_when_forced_redirection_for_specifiers() {
    let (tx, rx) = oneshot::channel::<()>();

    integration_test!(
        "./test_cases/main",
        NON_SECURE_PORT,
        "concurrent-redirect",
        None,
        None,
        None,
        None,
        (|resp| async {
            assert_eq!(resp.unwrap().status().as_u16(), 200);
            tx.send(()).unwrap();
        }),
        TerminationToken::new()
    );

    if timeout(Duration::from_secs(10), rx).await.is_err() {
        panic!("failed to check within 10 seconds");
    }
}

async fn test_allow_net<F, R>(allow_net: Option<Vec<&str>>, url: &str, callback: F)
where
    F: FnOnce(Result<Response, reqwest::Error>) -> R,
    R: Future<Output = ()>,
{
    let payload = serde_json::json!({
        "allowNet": allow_net,
        "url": url
    });

    let client = Client::new();
    let req = client
        .request(
            Method::POST,
            format!("http://localhost:{}/fetch", NON_SECURE_PORT),
        )
        .json(&payload)
        .build()
        .unwrap();

    integration_test!(
        "./test_cases/main_with_allow_net",
        NON_SECURE_PORT,
        "",
        None,
        None,
        Some(RequestBuilder::from_parts(client, req)),
        None,
        (|resp| async {
            callback(resp).await;
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_allow_net_fetch_google_com() {
    #[derive(Deserialize)]
    struct FetchResponse {
        status: u16,
        body: String,
    }

    // 1. allow only specific hosts
    test_allow_net(
        // because google.com redirects to www.google.com
        Some(vec!["google.com", "www.google.com"]),
        "https://google.com",
        |resp| async move {
            let resp = resp.unwrap();

            assert_eq!(resp.status().as_u16(), StatusCode::OK);

            let payload = resp.json::<FetchResponse>().await.unwrap();

            assert_eq!(payload.status, StatusCode::OK);
            assert!(!payload.body.is_empty());
        },
    )
    .await;

    // 2. allow only specific host (but not considering the redirected host)
    test_allow_net(
        Some(vec!["google.com"]),
        "https://google.com",
        |resp| async move {
            let resp = resp.unwrap();

            assert_eq!(resp.status().as_u16(), StatusCode::INTERNAL_SERVER_ERROR);

            let msg = resp.text().await.unwrap();

            assert_eq!(
                msg.as_str(),
                // google.com redirects to www.google.com, but we didn't allow it
                "PermissionDenied: Access to www.google.com is not allowed for user worker"
            );
        },
    )
    .await;

    // 3. deny all hosts
    test_allow_net(Some(vec![]), "https://google.com", |resp| async move {
        let resp = resp.unwrap();

        assert_eq!(resp.status().as_u16(), StatusCode::INTERNAL_SERVER_ERROR);

        let msg = resp.text().await.unwrap();

        assert_eq!(
            msg.as_str(),
            "PermissionDenied: Access to google.com is not allowed for user worker"
        );
    })
    .await;

    // 4. allow all hosts
    test_allow_net(None, "https://google.com", |resp| async move {
        let resp = resp.unwrap();

        assert_eq!(resp.status().as_u16(), StatusCode::OK);

        let payload = resp.json::<FetchResponse>().await.unwrap();

        assert_eq!(payload.status, StatusCode::OK);
        assert!(!payload.body.is_empty());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn test_fastify_v4_package() {
    integration_test!(
        "./test_cases/main",
        NON_SECURE_PORT,
        "fastify-v4",
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

#[tokio::test]
#[serial]
async fn test_fastify_latest_package() {
    integration_test!(
        "./test_cases/main",
        NON_SECURE_PORT,
        "fastify-latest",
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

#[tokio::test]
#[serial]
async fn test_declarative_style_fetch_handler() {
    integration_test!(
        "./test_cases/main",
        NON_SECURE_PORT,
        "serve-declarative-style",
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

#[tokio::test]
#[serial]
async fn test_issue_420() {
    integration_test!(
        "./test_cases/main",
        NON_SECURE_PORT,
        "issue-420",
        None,
        None,
        None,
        None,
        (|resp| async {
            let text = resp.unwrap().text().await.unwrap();

            assert!(text.starts_with("file:///"));
            assert!(text.ends_with(
                "/node_modules/localhost/@imagemagick/magick-wasm/0.0.30/dist/index.js"
            ));
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_issue_456() {
    let tb = TestBedBuilder::new("./test_cases/main").build().await;
    let resp = tb
        .request(|b| {
            b.uri("/issue-456")
                .header("x-context-source-map", "true")
                .body(Body::empty())
                .context("can't make request")
        })
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), StatusCode::OK);
    tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
}

#[tokio::test]
#[serial]
async fn test_should_render_detailed_failed_to_create_graph_error() {
    {
        integration_test!(
            "./test_cases/main",
            NON_SECURE_PORT,
            "graph-error-1",
            None,
            None,
            None,
            None,
            (|resp| async {
                let (payload, status) = ErrorResponsePayload::assert_error_response(resp).await;

                assert_eq!(status, 500);
                assert!(payload.msg.starts_with(
                    "InvalidWorkerCreation: worker boot error: failed to create the graph: \
                    Relative import path \"oak\" not prefixed with"
                ));
            }),
            TerminationToken::new()
        );
    }

    {
        integration_test!(
            "./test_cases/main",
            NON_SECURE_PORT,
            "graph-error-2",
            None,
            None,
            None,
            None,
            (|resp| async {
                let (payload, status) = ErrorResponsePayload::assert_error_response(resp).await;

                assert_eq!(status, 500);
                assert!(payload.msg.starts_with(
                    "InvalidWorkerCreation: worker boot error: failed to create the graph: \
                    Module not found \"file://"
                ));
            }),
            TerminationToken::new()
        );
    }
}

#[tokio::test]
#[serial]
async fn test_js_entrypoint() {
    {
        integration_test!(
            "./test_cases/main",
            NON_SECURE_PORT,
            "serve-js",
            None,
            None,
            None,
            None,
            (|resp| async {
                let resp = resp.unwrap();
                assert_eq!(resp.status().as_u16(), 200);
                let msg = resp.text().await.unwrap();
                assert_eq!(msg, "meow");
            }),
            TerminationToken::new()
        );
    }

    {
        integration_test!(
            "./test_cases/main",
            NON_SECURE_PORT,
            "serve-declarative-style-js",
            None,
            None,
            None,
            None,
            (|resp| async {
                let resp = resp.unwrap();
                assert_eq!(resp.status().as_u16(), 200);
                let msg = resp.text().await.unwrap();
                assert_eq!(msg, "meow");
            }),
            TerminationToken::new()
        );
    }
}

#[tokio::test]
#[serial]
async fn test_should_be_able_to_bundle_against_various_exts() {
    let get_eszip_buf = |path: &str| {
        let path = path.to_string();
        let mut emitter_factory = EmitterFactory::new();

        emitter_factory.set_jsx_import_source(JsxImportSourceConfig {
            default_specifier: Some("https://esm.sh/preact".to_string()),
            default_types_specifier: None,
            module: "jsx-runtime".to_string(),
            base_url: Url::from_file_path(std::env::current_dir().unwrap()).unwrap(),
        });

        async {
            generate_binary_eszip(
                PathBuf::from(path),
                Arc::new(emitter_factory),
                None,
                None,
                None,
            )
            .await
            .unwrap()
            .into_bytes()
        }
    };

    {
        let buf = get_eszip_buf("./test_cases/eszip-various-ext/npm-supabase/index.js").await;

        let client = Client::new();
        let req = client
            .request(
                Method::POST,
                format!("http://localhost:{}/meow", NON_SECURE_PORT),
            )
            .body(buf);

        integration_test!(
            "./test_cases/main_eszip",
            NON_SECURE_PORT,
            "",
            None,
            None,
            Some(req),
            None,
            (|resp| async {
                let resp = resp.unwrap();
                assert_eq!(resp.status().as_u16(), 200);
                let msg = resp.text().await.unwrap();
                assert_eq!(msg, "function");
            }),
            TerminationToken::new()
        );
    }

    let test_serve_simple_fn = |ext: &'static str, expected: &'static [u8]| {
        let ext = ext.to_string();
        let expected = expected.to_vec();

        async move {
            let buf = get_eszip_buf(&format!(
                "./test_cases/eszip-various-ext/serve/index.{}",
                ext
            ))
            .await;

            let client = Client::new();
            let req = client
                .request(
                    Method::POST,
                    format!("http://localhost:{}/meow", NON_SECURE_PORT),
                )
                .body(buf);

            integration_test!(
                "./test_cases/main_eszip",
                NON_SECURE_PORT,
                "",
                None,
                None,
                Some(req),
                None,
                (|resp| async move {
                    let resp = resp.unwrap();
                    assert_eq!(resp.status().as_u16(), 200);
                    let msg = resp.bytes().await.unwrap();
                    assert_eq!(msg, expected);
                }),
                TerminationToken::new()
            );
        }
    };

    test_serve_simple_fn("ts", b"meow").await;
    test_serve_simple_fn("js", b"meow").await;
    test_serve_simple_fn("mjs", b"meow").await;

    static REACT_RESULT: &str = r#"{"type":"div","props":{"children":"meow"},"__k":null,"__":null,"__b":0,"__e":null,"__c":null,"__v":-1,"__i":-1,"__u":0}"#;

    test_serve_simple_fn("jsx", REACT_RESULT.as_bytes()).await;
    test_serve_simple_fn("tsx", REACT_RESULT.as_bytes()).await;
}

#[tokio::test]
#[serial]
async fn test_private_npm_package_import() {
    // Required because test_cases/main_with_registry/registry/registry-handler.ts:58
    std::env::set_var("EDGE_RUNTIME_PORT", NON_SECURE_PORT.to_string());
    let _guard = scopeguard::guard((), |_| {
        std::env::remove_var("EDGE_RUNTIME_PORT");
    });

    let client = Client::new();
    let run_server_fn = |main: &'static str, token| async move {
        let (tx, mut rx) = mpsc::channel(1);
        let handle = tokio::task::spawn({
            async move {
                Server::new(
                    "127.0.0.1",
                    NON_SECURE_PORT,
                    None,
                    main.to_string(),
                    None,
                    None,
                    None,
                    None,
                    Default::default(),
                    Some(tx),
                    Default::default(),
                    Some(token),
                    vec![],
                    None,
                    None,
                    None,
                )
                .await
                .unwrap()
                .listen()
                .await
                .unwrap();
            }
        });

        let _ev = loop {
            match rx.recv().await {
                Some(health) => break health.into_listening().unwrap(),
                _ => continue,
            }
        };

        handle
    };

    {
        let token = TerminationToken::new();
        let handle = run_server_fn("./test_cases/main_with_registry", token.clone()).await;

        let resp = client
            .request(
                Method::GET,
                format!(
                    "http://localhost:{}/private-npm-package-import",
                    NON_SECURE_PORT
                ),
            )
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), 200);

        let body = resp.json::<serde_json::Value>().await.unwrap();
        let body = body.as_object().unwrap();

        assert_eq!(body.len(), 2);
        assert_eq!(body.get("meow"), Some(&json!("function")));
        assert_eq!(body.get("odd"), Some(&json!(true)));

        token.cancel();
        handle.await.unwrap();
    }

    {
        let token = TerminationToken::new();
        let handle = run_server_fn("./test_cases/main_with_registry", token.clone()).await;

        let resp = client
            .request(
                Method::GET,
                format!("http://localhost:{}/meow", NON_SECURE_PORT),
            )
            .header("x-service-path", "private-npm-package-import-2/inner")
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), 200);

        let body = resp.json::<serde_json::Value>().await.unwrap();
        let body = body.as_object().unwrap();

        assert_eq!(body.len(), 2);
        assert_eq!(body.get("meow"), Some(&json!("function")));
        assert_eq!(body.get("odd"), Some(&json!(true)));

        token.cancel();
        handle.await.unwrap();
    }

    {
        let token = TerminationToken::new();
        let handle = run_server_fn("./test_cases/main_eszip", token.clone()).await;

        let buf = {
            let mut emitter_factory = EmitterFactory::new();

            emitter_factory.set_npmrc_path("./test_cases/private-npm-package-import/.npmrc");

            generate_binary_eszip(
                PathBuf::from("./test_cases/private-npm-package-import/index.js"),
                Arc::new(emitter_factory),
                None,
                None,
                None,
            )
            .await
            .unwrap()
            .into_bytes()
        };

        let resp = client
            .request(
                Method::POST,
                format!("http://localhost:{}/meow", NON_SECURE_PORT),
            )
            .body(buf)
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), 200);

        let body = resp.json::<serde_json::Value>().await.unwrap();
        let body = body.as_object().unwrap();

        assert_eq!(body.len(), 2);
        assert_eq!(body.get("meow"), Some(&json!("function")));
        assert_eq!(body.get("odd"), Some(&json!(true)));

        token.cancel();
        handle.await.unwrap();
    }
}

#[tokio::test]
#[serial]
async fn test_tmp_fs_usage() {
    {
        integration_test!(
            "./test_cases/main",
            NON_SECURE_PORT,
            "use-tmp-fs",
            None,
            None,
            None,
            None,
            (|resp| async {
                let resp = resp.unwrap();

                assert_eq!(resp.status().as_u16(), 200);

                let body = resp.json::<serde_json::Value>().await.unwrap();
                let body = body.as_object().unwrap();

                assert_eq!(body.len(), 4);
                assert_eq!(body.get("written"), Some(&json!(8)));
                assert_eq!(body.get("content"), Some(&json!("meowmeow")));
                assert_eq!(body.get("deleted"), Some(&json!(true)));

                let steps = body.get("steps").unwrap().as_array().unwrap();

                assert_eq!(&steps[0], &json!(true));
                assert_eq!(&steps[1], &json!(false));
            }),
            TerminationToken::new()
        );
    }

    {
        integration_test!(
            "./test_cases/main",
            NON_SECURE_PORT,
            "use-tmp-fs-2",
            None,
            None,
            None,
            None,
            (|resp| async {
                let resp = resp.unwrap();

                assert_eq!(resp.status().as_u16(), 200);

                let body = resp.json::<serde_json::Value>().await.unwrap();
                let body = body.as_object().unwrap();

                assert_eq!(body.len(), 2);
                assert_eq!(body.get("hadExisted"), Some(&json!(true)));

                let path = body.get("path").unwrap().as_str().unwrap();
                let f = fs::read(path).await.unwrap();
                let mut cursor = Cursor::new(&f);

                let client = Client::new();
                let resp2 = client
                    .request(Method::GET, "https://httpbin.org/stream/20".to_string())
                    .send()
                    .await
                    .unwrap();

                assert_eq!(resp2.status().as_u16(), 200);

                let body2 = resp2.bytes().await.unwrap();
                let mut cursor2 = Cursor::new(&*body2);
                let mut count = 0;

                loop {
                    use serde_json::*;

                    let mut buf = String::new();

                    cursor.read_line(&mut buf).unwrap();
                    let mut msg = from_str::<Map<String, Value>>(&buf).unwrap();

                    buf.clear();
                    cursor2.read_line(&mut buf).unwrap();
                    let mut msg2 = from_str::<Map<String, Value>>(&buf).unwrap();

                    assert!(msg.remove("headers").is_some());
                    assert!(msg2.remove("headers").is_some());
                    assert_eq!(msg, msg2);

                    count += 1;
                    if count >= 20 {
                        break;
                    }
                }
            }),
            TerminationToken::new()
        );
    }
}

#[tokio::test]
#[serial]
async fn test_tmp_fs_should_not_be_available_in_import_stmt() {
    // The s3 fs and tmp fs are not currently attached to the module loader, so the import statement
    // should not recognize their prefixes. (But, depending on the case, they may be attached in the
    // future)
    integration_test!(
        "./test_cases/main",
        NON_SECURE_PORT,
        "use-tmp-fs-in-import-stmt",
        None,
        None,
        None,
        None,
        (|resp| async {
            let (payload, status) = ErrorResponsePayload::assert_error_response(resp).await;

            assert_eq!(status, 500);
            dbg!(&payload.msg);
            assert!(payload.msg.starts_with(
                "InvalidWorkerResponse: event loop error while evaluating the module: \
                TypeError: Module not found: file:///tmp/meowmeow.ts"
            ));
        }),
        TerminationToken::new()
    );
}

// -- sb_ai: ORT @huggingface/transformers
async fn test_ort_transformers_js(script_path: &str) {
    fn visit_json(value: &mut serde_json::Value) {
        use serde_json::Number;
        use serde_json::Value::*;

        match value {
            Array(vec) => {
                for v in vec {
                    visit_json(v);
                }
            }
            Object(map) => {
                for (_, v) in map {
                    visit_json(v);
                }
            }
            Number(number) => {
                if let Some(f) = number.as_f64() {
                    *number = Number::from_f64((f * 1_000_000.0).round() / 1_000_000.0).unwrap();
                }
            }

            _ => {}
        }
    }

    use std::env::consts;

    let base_path = "./test_cases/ai-ort-rust-backend";
    let main_path = format!("{}/main", base_path);
    let script_path = format!("transformers-js/{}", script_path);
    let snapshot_path = PathBuf::from(base_path)
        .join(script_path.as_str())
        .join(format!("__snapshot__/{}_{}.json", consts::OS, consts::ARCH));

    let client = Client::new();
    let body = {
        if snapshot_path.exists() {
            tokio::fs::read(&snapshot_path).await.unwrap()
        } else {
            b"null".to_vec()
        }
    };

    let content_length = body.len();
    let req = client
        .request(
            Method::POST,
            format!(
                "http://localhost:{}/{}",
                NON_SECURE_PORT,
                script_path.as_str(),
            ),
        )
        .body(body)
        .header("Content-Type", "application/json")
        .header("Content-Length", content_length.to_string());

    integration_test!(
        main_path,
        NON_SECURE_PORT,
        "",
        None,
        None,
        Some(req),
        None,
        (|resp| async {
            let res = resp.unwrap();
            let status_code = res.status();

            assert!(matches!(status_code, StatusCode::OK | StatusCode::CREATED));

            if status_code == StatusCode::OK {
                return;
            }

            assert_eq!(std::env::var("CI").ok(), None);
            assert!(!snapshot_path.exists());

            tokio::fs::create_dir_all(snapshot_path.parent().unwrap())
                .await
                .unwrap();

            let mut body = res.json::<serde_json::Value>().await.unwrap();
            let mut file = fs::File::create(&snapshot_path).await.unwrap();

            visit_json(&mut body);

            let content = serde_json::to_vec(&body).unwrap();

            file.write_all(&content).await.unwrap();
        }),
        TerminationToken::new()
    );
}

#[tokio::test]
#[serial]
async fn test_ort_nlp_feature_extraction() {
    test_ort_transformers_js("feature-extraction").await;
}

#[tokio::test]
#[serial]
async fn test_ort_nlp_fill_mask() {
    test_ort_transformers_js("fill-mask").await;
}

#[tokio::test]
#[serial]
async fn test_ort_nlp_question_answering() {
    test_ort_transformers_js("question-answering").await;
}

#[tokio::test]
#[serial]
async fn test_ort_nlp_summarization() {
    test_ort_transformers_js("summarization").await;
}

#[tokio::test]
#[serial]
async fn test_ort_nlp_text_classification() {
    test_ort_transformers_js("text-classification").await;
}

#[tokio::test]
#[serial]
async fn test_ort_nlp_text_generation() {
    test_ort_transformers_js("text-generation").await;
}

#[tokio::test]
#[serial]
async fn test_ort_nlp_text2text_generation() {
    test_ort_transformers_js("text2text-generation").await;
}

#[tokio::test]
#[serial]
async fn test_ort_nlp_token_classification() {
    test_ort_transformers_js("token-classification").await;
}

#[tokio::test]
#[serial]
async fn test_ort_nlp_translation() {
    test_ort_transformers_js("translation").await;
}

#[tokio::test]
#[serial]
async fn test_ort_nlp_zero_shot_classification() {
    test_ort_transformers_js("zero-shot-classification").await;
}

#[tokio::test]
#[serial]
async fn test_ort_vision_image_feature_extraction() {
    test_ort_transformers_js("image-feature-extraction").await;
}

#[tokio::test]
#[serial]
async fn test_ort_vision_image_classification() {
    test_ort_transformers_js("image-classification").await;
}

#[tokio::test]
#[serial]
async fn test_ort_vision_zero_shot_image_classification() {
    test_ort_transformers_js("zero-shot-image-classification").await;
}

// -- sb_ai(cache): ORT @huggingface/transformers
#[tokio::test]
#[serial]
async fn test_ort_cache_nlp_feature_extraction() {
    test_ort_transformers_js("feature-extraction-cache").await;
}

#[tokio::test]
#[serial]
async fn test_ort_cache_nlp_fill_mask() {
    test_ort_transformers_js("fill-mask-cache").await;
}

#[tokio::test]
#[serial]
async fn test_ort_cache_nlp_question_answering() {
    test_ort_transformers_js("question-answering-cache").await;
}

#[tokio::test]
#[serial]
async fn test_ort_cache_nlp_summarization() {
    test_ort_transformers_js("summarization-cache").await;
}

#[tokio::test]
#[serial]
async fn test_ort_cache_nlp_text_classification() {
    test_ort_transformers_js("text-classification-cache").await;
}

#[tokio::test]
#[serial]
async fn test_ort_cache_nlp_text_generation() {
    test_ort_transformers_js("text-generation-cache").await;
}

#[tokio::test]
#[serial]
async fn test_ort_cache_nlp_text2text_generation() {
    test_ort_transformers_js("text2text-generation-cache").await;
}

#[tokio::test]
#[serial]
async fn test_ort_cache_nlp_token_classification() {
    test_ort_transformers_js("token-classification-cache").await;
}

#[tokio::test]
#[serial]
async fn test_ort_cache_nlp_translation() {
    test_ort_transformers_js("translation-cache").await;
}

#[tokio::test]
#[serial]
async fn test_ort_cache_nlp_zero_shot_classification() {
    test_ort_transformers_js("zero-shot-classification-cache").await;
}

#[tokio::test]
#[serial]
async fn test_ort_cache_vision_image_feature_extraction() {
    test_ort_transformers_js("image-feature-extraction-cache").await;
}

#[tokio::test]
#[serial]
async fn test_ort_cache_vision_image_classification() {
    test_ort_transformers_js("image-classification-cache").await;
}

#[tokio::test]
#[serial]
async fn test_ort_cache_vision_zero_shot_image_classification() {
    test_ort_transformers_js("zero-shot-image-classification-cache").await;
}

async fn test_runtime_beforeunload_event(kind: &'static str, pct: u8) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let tb = TestBedBuilder::new("./test_cases/runtime-event")
        .with_per_worker_policy(None)
        .with_worker_event_sender(Some(tx))
        .with_server_flags(ServerFlags {
            beforeunload_wall_clock_pct: Some(pct),
            beforeunload_cpu_pct: Some(pct),
            beforeunload_memory_pct: Some(pct),
            ..Default::default()
        })
        .build()
        .await;

    let resp = tb
        .request(|b| {
            b.uri(format!("/{}", kind))
                .method("GET")
                .body(Body::empty())
                .context("can't make request")
        })
        .await
        .unwrap();

    assert_ne!(resp.status().as_u16(), StatusCode::OK);

    tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;

    while let Some(ev) = rx.recv().await {
        let WorkerEvents::Log(ev) = ev.event else {
            continue;
        };
        if ev.level != LogLevel::Info {
            continue;
        }
        if ev
            .msg
            .contains(&format!("triggered {}", kind.replace('-', "_")))
        {
            return;
        }
    }

    unreachable!("test failed");
}

#[tokio::test]
#[serial]
async fn test_runtime_event_beforeunload_cpu() {
    test_runtime_beforeunload_event("cpu", 50).await;
}

#[tokio::test]
#[serial]
async fn test_runtime_event_beforeunload_wall_clock() {
    test_runtime_beforeunload_event("wall-clock", 50).await;
}

#[tokio::test]
#[serial]
async fn test_runtime_event_beforeunload_mem() {
    test_runtime_beforeunload_event("mem", 50).await;
}

// NOTE(Nyannyacha): We cannot enable this test unless we clarify the trigger point of the unload
// event.
//
// #[tokio::test]
// #[serial]
// async fn test_runtime_event_unload() {
//     let (tx, mut rx) = mpsc::unbounded_channel();
//     let tb = TestBedBuilder::new("./test_cases/runtime-event")
//         .with_per_worker_policy(None)
//         .with_worker_event_sender(Some(tx))
//         .build()
//         .await;
//
//     let resp = tb
//         .request(|b| {
//             b.uri("/unload")
//                 .method("GET")
//                 .body(Body::empty())
//                 .context("can't make request")
//         })
//         .await
//         .unwrap();
//
//     assert_eq!(resp.status().as_u16(), StatusCode::OK);
//
//     sleep(Duration::from_secs(8)).await;
//     tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
//
//     while let Some(ev) = rx.recv().await {
//         let WorkerEvents::Log(ev) = ev.event else {
//             continue;
//         };
//         if ev.level != LogLevel::Info {
//             continue;
//         }
//         if ev.msg.contains("triggered unload") {
//             break;
//         }
//     }
//
//     unreachable!("test failed");
// }

#[tokio::test]
#[serial]
async fn test_should_wait_for_background_tests() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let tb = TestBedBuilder::new("./test_cases/main")
        // only the `per_worker` policy allows waiting for background tasks.
        .with_per_worker_policy(None)
        .with_worker_event_sender(Some(tx))
        .build()
        .await;

    let resp = tb
        .request(|b| {
            b.uri("/mark-background-task")
                .header("x-cpu-time-soft-limit-ms", HeaderValue::from_static("100"))
                .body(Body::empty())
                .context("can't make request")
        })
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), StatusCode::OK);

    tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;

    while let Some(ev) = rx.recv().await {
        let WorkerEvents::Log(ev) = ev.event else {
            continue;
        };
        if ev.level != LogLevel::Info {
            continue;
        }
        if ev.msg.contains("meow") {
            return;
        }
    }

    unreachable!("test failed");
}

#[tokio::test]
#[serial]
async fn test_should_not_wait_for_background_tests() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let tb = TestBedBuilder::new("./test_cases/main")
        // only the `per_worker` policy allows waiting for background tasks.
        .with_per_worker_policy(None)
        .with_worker_event_sender(Some(tx))
        .build()
        .await;

    let resp = tb
        .request(|b| {
            b.uri("/mark-background-task-2")
                .header("x-cpu-time-soft-limit-ms", HeaderValue::from_static("100"))
                .body(Body::empty())
                .context("can't make request")
        })
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), StatusCode::OK);

    tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;

    while let Some(ev) = rx.recv().await {
        let WorkerEvents::Log(ev) = ev.event else {
            continue;
        };
        if ev.level != LogLevel::Info {
            continue;
        }
        if ev.msg.contains("meow") {
            unreachable!("test failed");
        }
    }
}

#[tokio::test]
#[serial]
async fn test_should_be_able_to_trigger_early_drop_with_wall_clock() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let tb = TestBedBuilder::new("./test_cases/main")
        .with_per_worker_policy(None)
        .with_worker_event_sender(Some(tx))
        .build()
        .await;

    let resp = tb
        .request(|b| {
            b.uri("/early-drop-wall-clock")
                .header("x-worker-timeout-ms", HeaderValue::from_static("3000"))
                .body(Body::empty())
                .context("can't make request")
        })
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), StatusCode::OK);

    sleep(Duration::from_secs(2)).await;
    rx.close();
    tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;

    while let Some(ev) = rx.recv().await {
        let WorkerEvents::Log(ev) = ev.event else {
            continue;
        };
        if ev.level != LogLevel::Info {
            continue;
        }
        if ev.msg.contains("early_drop") {
            return;
        }
    }

    unreachable!("test failed");
}

#[tokio::test]
#[serial]
async fn test_should_be_able_to_trigger_early_drop_with_mem() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let tb = TestBedBuilder::new("./test_cases/main")
        .with_per_worker_policy(None)
        .with_worker_event_sender(Some(tx))
        .build()
        .await;

    let resp = tb
        .request(|b| {
            b.uri("/early-drop-mem")
                .header("x-memory-limit-mb", HeaderValue::from_static("20"))
                .body(Body::empty())
                .context("can't make request")
        })
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), StatusCode::OK);

    sleep(Duration::from_secs(2)).await;
    rx.close();
    tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;

    while let Some(ev) = rx.recv().await {
        let WorkerEvents::Log(ev) = ev.event else {
            continue;
        };
        if ev.level != LogLevel::Info {
            continue;
        }
        if ev.msg.contains("early_drop") {
            return;
        }
    }

    unreachable!("test failed");
}

#[derive(Deserialize)]
struct ErrorResponsePayload {
    msg: String,
}

impl ErrorResponsePayload {
    async fn assert_error_response(resp: Result<Response, reqwest::Error>) -> (Self, u16) {
        let res = resp.unwrap();
        let status = res.status().as_u16();
        let res = res.json::<Self>().await;

        assert!(res.is_ok());

        (res.unwrap(), status)
    }
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
