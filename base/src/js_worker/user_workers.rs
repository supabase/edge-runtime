use crate::worker_ctx::{CreateUserWorkerResult, UserWorkerOptions, WorkerPoolMsg};

use deno_core::error::{custom_error, AnyError};
use deno_core::include_js_files;
use deno_core::op;
use deno_core::Extension;
use deno_core::OpState;
use deno_core::{ByteString, ZeroCopyBuf};
use hyper::header::{HeaderName, HeaderValue};
use hyper::{Body, Request, Response};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use tokio::sync::{mpsc, oneshot};

pub fn init() -> Extension {
    Extension::builder("custom:user_workers")
        .js(include_js_files!(
          prefix "custom:ext/user_workers",
          "js/user_workers.js",
        ))
        .ops(vec![
            op_user_worker_create::decl(),
            op_user_worker_fetch::decl(),
        ])
        .build()
}

#[op]
pub async fn op_user_worker_create(
    state: Rc<RefCell<OpState>>,
    service_path: String,
    memory_limit_mb: u64,
    worker_timeout_ms: u64,
    no_module_cache: bool,
    import_map_path: Option<String>,
    env_vars_vec: Vec<(String, String)>,
) -> Result<String, AnyError> {
    let op_state = state.borrow();
    let tx = op_state.borrow::<mpsc::UnboundedSender<WorkerPoolMsg>>();
    let (result_tx, result_rx) = oneshot::channel::<CreateUserWorkerResult>();

    let mut env_vars = HashMap::new();
    for (key, value) in env_vars_vec {
        env_vars.insert(key, value);
    }

    let user_worker_options = UserWorkerOptions {
        service_path: PathBuf::from(service_path),
        memory_limit_mb,
        worker_timeout_ms,
        no_module_cache,
        import_map_path,
        env_vars,
    };
    tx.send(WorkerPoolMsg::CreateUserWorker(
        user_worker_options,
        result_tx,
    ));

    let result = result_rx.await;
    if result.is_err() {
        return Err(custom_error(
            "create_user_worker_error",
            "failed to create worker",
        ));
    }

    let result = result.unwrap();
    Ok(result.key)
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UserWorkerRequest {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserWorkerResponse {
    status: u16,
    status_text: String,
    headers: Vec<(ByteString, ByteString)>,
}

#[op]
pub async fn op_user_worker_fetch(
    state: Rc<RefCell<OpState>>,
    key: String,
    req: UserWorkerRequest,
) -> Result<UserWorkerResponse, AnyError> {
    let op_state = state.borrow();
    let tx = op_state.borrow::<mpsc::UnboundedSender<WorkerPoolMsg>>();
    let (result_tx, result_rx) = oneshot::channel::<Response<Body>>();

    let mut request = Request::builder()
        .uri(req.url)
        .method(req.method.as_str())
        .body(hyper::Body::empty())
        .unwrap();

    // set the request headers
    for (key, value) in req.headers {
        if !key.is_empty() {
            let header_name = HeaderName::try_from(key).unwrap();
            let header_value = HeaderValue::try_from(value).unwrap_or(HeaderValue::from_static(""));

            // skip content-length header, this would be re-calculated and added to the request
            if header_name.eq("content-length") {
                continue;
            }

            request.headers_mut().append(header_name, header_value);
        }
    }

    tx.send(WorkerPoolMsg::SendRequestToWorker(key, request, result_tx));

    let result = result_rx.await;
    if result.is_err() {
        return Err(custom_error(
            "user_worker_fetch",
            "failed to fetch from user worker",
        ));
    }

    let result = result.unwrap();

    let mut headers = vec![];
    for (key, value) in result.headers().iter() {
        headers.push((
            ByteString::from(key.as_str()),
            ByteString::from(value.to_str().unwrap_or(&"")),
        ));
    }

    let response = UserWorkerResponse {
        status: result.status().as_u16(),
        status_text: result
            .status()
            .canonical_reason()
            .unwrap_or(&"<unknown status code>")
            .to_string(),
        headers,
    };
    Ok(response)
}
