use crate::worker_ctx::{CreateUserWorkerResult, UserWorkerOptions, WorkerPoolMsg};

use deno_core::error::{custom_error, type_error, AnyError};
use deno_core::futures::stream::Peekable;
use deno_core::futures::Stream;
use deno_core::futures::StreamExt;
use deno_core::include_js_files;
use deno_core::op;
use deno_core::AsyncRefCell;
use deno_core::CancelTryFuture;
use deno_core::Extension;
use deno_core::OpState;
use deno_core::Resource;
use deno_core::ResourceId;
use deno_core::{AsyncResult, BufView, ByteString, CancelHandle, RcRef};
use hyper::body::HttpBody;
use hyper::header::{HeaderName, HeaderValue};
use hyper::{Body, Request, Response};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
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
    has_body: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserWorkerResponse {
    status: u16,
    status_text: String,
    headers: Vec<(ByteString, ByteString)>,
    body_rid: ResourceId,
}

type BytesStream = Pin<Box<dyn Stream<Item = Result<bytes::Bytes, std::io::Error>> + Unpin>>;

struct UserWorkerRquestBodyResource {
    writer: AsyncRefCell<Peekable<BytesStream>>,
    cancel: CancelHandle,
    size: Option<u64>,
}

struct UserWorkerResponseBodyResource {
    reader: AsyncRefCell<Peekable<BytesStream>>,
    cancel: CancelHandle,
    size: Option<u64>,
}

impl Resource for UserWorkerResponseBodyResource {
    fn name(&self) -> Cow<str> {
        "userWorkerResponseBody".into()
    }

    fn read(self: Rc<Self>, limit: usize) -> AsyncResult<BufView> {
        Box::pin(async move {
            let reader = RcRef::map(&self, |r| &r.reader).borrow_mut().await;

            let fut = async move {
                let mut reader = Pin::new(reader);
                loop {
                    match reader.as_mut().peek_mut().await {
                        Some(Ok(chunk)) if !chunk.is_empty() => {
                            let len = std::cmp::min(limit, chunk.len());
                            let chunk = chunk.split_to(len);
                            break Ok(chunk.into());
                        }
                        // This unwrap is safe because `peek_mut()` returned `Some`, and thus
                        // currently has a peeked value that can be synchronously returned
                        // from `next()`.
                        //
                        // The future returned from `next()` is always ready, so we can
                        // safely call `await` on it without creating a race condition.
                        Some(_) => match reader.as_mut().next().await.unwrap() {
                            Ok(chunk) => assert!(chunk.is_empty()),
                            Err(err) => break Err(type_error(err.to_string())),
                        },
                        None => break Ok(BufView::empty()),
                    }
                }
            };

            let cancel_handle = RcRef::map(self, |r| &r.cancel);
            fut.try_or_cancel(cancel_handle).await
        })
    }

    fn close(self: Rc<Self>) {
        self.cancel.cancel()
    }

    fn size_hint(&self) -> (u64, Option<u64>) {
        (self.size.unwrap_or(0), self.size)
    }
}

#[op]
pub async fn op_user_worker_fetch(
    state: Rc<RefCell<OpState>>,
    key: String,
    req: UserWorkerRequest,
) -> Result<UserWorkerResponse, AnyError> {
    let mut op_state = state.borrow_mut();
    let tx = op_state.borrow::<mpsc::UnboundedSender<WorkerPoolMsg>>();
    let (result_tx, result_rx) = oneshot::channel::<Response<Body>>();

    let mut body = Body::empty();
    if req.has_body {
        //body = Body::wrap_stream(stream);
    }

    let mut request = Request::builder()
        .uri(req.url)
        .method(req.method.as_str())
        .body(body)
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

    let status = result.status().as_u16();
    let status_text = result
        .status()
        .canonical_reason()
        .unwrap_or(&"<unknown status code>")
        .to_string();

    let size = HttpBody::size_hint(result.body()).exact();
    let stream: BytesStream = Box::pin(
        result
            .into_body()
            .map(|r| r.map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))),
    );
    let body_rid = op_state.resource_table.add(UserWorkerResponseBodyResource {
        reader: AsyncRefCell::new(stream.peekable()),
        cancel: CancelHandle::default(),
        size,
    });

    let response = UserWorkerResponse {
        status,
        status_text,
        headers,
        body_rid,
    };
    Ok(response)
}
