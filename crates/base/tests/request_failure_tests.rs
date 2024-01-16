use anyhow::Context;
use http::{Request, StatusCode};
use hyper::{body::to_bytes, Body};
use tokio::join;

use crate::integration_test_helper::TestBedBuilder;

#[path = "../src/utils/integration_test_helper.rs"]
mod integration_test_helper;

#[tokio::test]
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
