use std::time::Duration;

use anyhow::Context;
use base::{integration_test, rt_worker::worker_ctx::TerminationToken, server::ServerEvent};
use http::{Request, StatusCode};
use hyper::{body::to_bytes, Body};
use serial_test::serial;
use tokio::{join, sync::mpsc};

use crate::integration_test_helper::TestBedBuilder;

#[path = "../src/utils/integration_test_helper.rs"]
mod integration_test_helper;

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

    tb.exit().await;
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

    tb.exit().await;
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

    tb.exit().await;
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

    tb.exit().await;
}

#[tokio::test]
#[serial]
async fn req_failure_case_intentional_peer_reset() {
    let termination_token = TerminationToken::new();
    let (server_ev_tx, mut server_ev_rx) = mpsc::unbounded_channel();

    integration_test!(
        "./test_cases/main",
        8999,
        "slow_resp",
        None,
        None,
        None::<reqwest::RequestBuilder>,
        (
            |port: usize,
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
                    reqwest::Client::new()
                        .get(format!("http://localhost:{}/{}", port, url))
                        .timeout(Duration::from_millis(100))
                        .send()
                        .await,
                )
            },
            |_resp: Result<reqwest::Response, reqwest::Error>| async {}
        ),
        termination_token.clone()
    );

    let ev = loop {
        tokio::select! {
            Some(ev) = server_ev_rx.recv() => break ev,
            else => continue,
        }
    };

    assert!(matches!(ev, ServerEvent::ConnectionError(e) if e.is_incomplete_message()));

    termination_token.cancel_and_wait().await;
}
