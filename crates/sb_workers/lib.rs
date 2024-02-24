pub mod context;
pub mod errors;

use crate::context::{
    CreateUserWorkerResult, UserWorkerMsgs, UserWorkerRuntimeOpts, WorkerContextInitOpts,
    WorkerRuntimeOpts,
};
use anyhow::Error;
use context::SendRequestResult;
use deno_core::error::{custom_error, type_error, AnyError};
use deno_core::futures::stream::Peekable;
use deno_core::futures::{FutureExt, Stream, StreamExt};
use deno_core::op2;
use deno_core::{
    AsyncRefCell, AsyncResult, BufView, ByteString, CancelFuture, CancelHandle, CancelTryFuture,
    JsBuffer, OpState, RcRef, Resource, ResourceId, WriteOutcome,
};
use deno_http::{HttpRequestReader, HttpStreamResource};
use errors::WorkerError;
use http_utils::utils::get_upgrade_type;
use hyper::body::HttpBody;
use hyper::header::{HeaderName, HeaderValue, CONTENT_LENGTH};
use hyper::upgrade::OnUpgrade;
use hyper::{Body, Method, Request};
use log::error;
use sb_core::conn_sync::{ConnSync, ConnWatcher};
use sb_graph::EszipPayloadKind;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

deno_core::extension!(
    sb_user_workers,
    ops = [
        op_user_worker_create,
        op_user_worker_fetch_build,
        op_user_worker_fetch_send,
    ],
    esm_entry_point = "ext:sb_user_workers/user_workers.js",
    esm = ["user_workers.js",]
);

#[derive(Deserialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UserWorkerCreateOptions {
    service_path: String,
    no_module_cache: bool,
    import_map_path: Option<String>,
    env_vars: Vec<(String, String)>,
    force_create: bool,
    allow_remote_modules: bool,
    net_access_disabled: bool,
    custom_module_root: Option<String>,
    maybe_eszip: Option<JsBuffer>,
    maybe_entrypoint: Option<String>,
    maybe_module_code: Option<String>,

    memory_limit_mb: u64,
    low_memory_multiplier: u64,
    worker_timeout_ms: u64,
    cpu_time_soft_limit_ms: u64,
    cpu_time_hard_limit_ms: u64,
}

#[op2(async)]
#[string]
pub async fn op_user_worker_create(
    state: Rc<RefCell<OpState>>,
    #[serde] opts: UserWorkerCreateOptions,
) -> Result<String, AnyError> {
    let result_rx = {
        let op_state = state.borrow();
        let tx = op_state.borrow::<mpsc::UnboundedSender<UserWorkerMsgs>>();
        let (result_tx, result_rx) = oneshot::channel::<Result<CreateUserWorkerResult, Error>>();

        let UserWorkerCreateOptions {
            service_path,
            no_module_cache,
            import_map_path,
            env_vars,
            force_create,
            net_access_disabled,
            allow_remote_modules,
            custom_module_root,
            maybe_eszip,
            maybe_entrypoint,
            maybe_module_code,

            memory_limit_mb,
            low_memory_multiplier,
            worker_timeout_ms,
            cpu_time_soft_limit_ms,
            cpu_time_hard_limit_ms,
        } = opts;

        let mut env_vars_map = HashMap::new();
        for (key, value) in env_vars {
            env_vars_map.insert(key, value);
        }

        let user_worker_options = WorkerContextInitOpts {
            service_path: PathBuf::from(service_path),
            no_module_cache,
            import_map_path,
            env_vars: env_vars_map,
            events_rx: None,
            timing: None,
            maybe_eszip: maybe_eszip.map(EszipPayloadKind::JsBufferKind),
            maybe_entrypoint,
            maybe_module_code: maybe_module_code.map(|v| v.into()),
            conf: WorkerRuntimeOpts::UserWorker(UserWorkerRuntimeOpts {
                memory_limit_mb,
                low_memory_multiplier,
                worker_timeout_ms,
                cpu_time_soft_limit_ms,
                cpu_time_hard_limit_ms,
                force_create,
                net_access_disabled,
                allow_remote_modules,
                custom_module_root,
                key: None,
                pool_msg_tx: None,
                events_msg_tx: None,
                cancel: None,
                service_path: None,
            }),
        };

        tx.send(UserWorkerMsgs::Create(user_worker_options, result_tx))?;
        result_rx
    };

    let result = result_rx.await;
    if result.is_err() {
        return Err(custom_error(
            "InvalidWorkerCreation",
            "failed to create worker",
        ));
    }

    // channel returns a Result<T, E>, we need to unwrap it first;
    let result = result.unwrap();
    match result {
        Err(e) => Err(custom_error("InvalidWorkerCreation", e.to_string())),
        Ok(res) => Ok(res.key.to_string()),
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UserWorkerRequest {
    method: ByteString,
    url: String,
    headers: Vec<(String, String)>,
    has_body: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserWorkerBuiltRequest {
    request_rid: ResourceId,
    request_body_rid: Option<ResourceId>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserWorkerResponse {
    status: u16,
    status_text: String,
    headers: Vec<(ByteString, ByteString)>,
    body_rid: ResourceId,
    size: Option<u64>,
}

struct UserWorkerRequestResource(Request<Body>);

impl Resource for UserWorkerRequestResource {
    fn name(&self) -> Cow<str> {
        "userWorkerRequest".into()
    }
}

struct UserWorkerRequestBodyResource {
    body: AsyncRefCell<Option<mpsc::Sender<Result<bytes::Bytes, Error>>>>,
    cancel: CancelHandle,
}

impl Resource for UserWorkerRequestBodyResource {
    fn name(&self) -> Cow<str> {
        "userWorkerRequestBody".into()
    }

    fn write(self: Rc<Self>, buf: BufView) -> AsyncResult<WriteOutcome> {
        Box::pin(async move {
            let bytes: bytes::Bytes = buf.into();
            let nwritten = bytes.len();
            let body = RcRef::map(&self, |r| &r.body).borrow_mut().await;
            let body = (*body).as_ref();
            let cancel = RcRef::map(self, |r| &r.cancel);
            let body = body.ok_or(type_error(
                "request body receiver not connected (request closed)",
            ))?;

            body.send(Ok(bytes))
                .or_cancel(cancel)
                .await?
                .map_err(|_| type_error("request body receiver not connected (request closed)"))?;

            Ok(WriteOutcome::Full { nwritten })
        })
    }

    fn write_error(self: Rc<Self>, error: Error) -> AsyncResult<()> {
        async move {
            let body = RcRef::map(&self, |r| &r.body).borrow_mut().await;
            let body = (*body).as_ref();
            let cancel = RcRef::map(self, |r| &r.cancel);
            let body = body.ok_or(type_error(
                "request body receiver not connected (request closed)",
            ))?;
            body.send(Err(error)).or_cancel(cancel).await??;
            Ok(())
        }
        .boxed_local()
    }

    fn shutdown(self: Rc<Self>) -> AsyncResult<()> {
        async move {
            let mut body = RcRef::map(&self, |r| &r.body).borrow_mut().await;
            body.take();
            Ok(())
        }
        .boxed_local()
    }

    fn close(self: Rc<Self>) {
        self.cancel.cancel();
    }
}

type BytesStream = Pin<Box<dyn Stream<Item = Result<bytes::Bytes, std::io::Error>> + Unpin>>;

struct UserWorkerResponseBodyResource {
    reader: AsyncRefCell<Peekable<BytesStream>>,
    cancel: CancelHandle,
    size: Option<u64>,
    req_end_tx: mpsc::UnboundedSender<()>,
    conn_watch: Option<watch::Receiver<ConnSync>>,
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
        self.cancel.cancel();

        let _ = self.req_end_tx.send(());
        let Ok(this) = Rc::try_unwrap(self) else {
            return;
        };

        tokio::spawn(async move {
            if let Some(mut watch) = this.conn_watch.clone() {
                match watch.wait_for(|it| *it == ConnSync::Recv).await {
                    Ok(_) => {}
                    Err(ex) => error!(
                        "error while waiting for the outbound connection to be finished: {}",
                        ex.to_string()
                    ),
                }
            }
        });
    }

    fn size_hint(&self) -> (u64, Option<u64>) {
        (self.size.unwrap_or(0), self.size)
    }
}

#[op2]
#[serde]
pub fn op_user_worker_fetch_build(
    state: &mut OpState,
    #[serde] req: UserWorkerRequest,
) -> Result<UserWorkerBuiltRequest, AnyError> {
    let method = Method::from_bytes(&req.method)?;

    let mut builder = Request::builder().uri(req.url).method(&method);
    let mut body = Body::empty();
    let mut request_body_rid = None;

    if req.has_body {
        let (tx, stream) = mpsc::channel(1);

        body = Body::wrap_stream(BodyStream(stream));
        request_body_rid = Some(state.resource_table.add(UserWorkerRequestBodyResource {
            body: AsyncRefCell::new(Some(tx)),
            cancel: CancelHandle::default(),
        }));
    }

    // set the request headers
    for (key, value) in req.headers {
        if !key.is_empty() {
            let header_name = HeaderName::try_from(key).unwrap();
            let mut header_value =
                HeaderValue::try_from(value).unwrap_or(HeaderValue::from_static(""));

            // if request has no body explicitly set the content-length to 0
            if !req.has_body
                && header_name == CONTENT_LENGTH
                && matches!(method, Method::POST | Method::PUT)
            {
                header_value = HeaderValue::from(0);
            }

            builder = builder.header(header_name, header_value);
        }
    }

    let req = builder.body(body)?;
    let request_rid = state.resource_table.add(UserWorkerRequestResource(req));

    Ok(UserWorkerBuiltRequest {
        request_rid,
        request_body_rid,
    })
}

#[op2(async)]
#[serde]
pub async fn op_user_worker_fetch_send(
    state: Rc<RefCell<OpState>>,
    #[string] key: String,
    #[smi] rid: ResourceId,
    #[smi] stream_rid: ResourceId,
    #[smi] watcher_rid: Option<ResourceId>,
) -> Result<UserWorkerResponse, AnyError> {
    let (tx, req) = {
        let (tx, mut req) = {
            let mut op_state = state.borrow_mut();
            let tx = op_state
                .borrow::<mpsc::UnboundedSender<UserWorkerMsgs>>()
                .clone();

            let req = Rc::try_unwrap(
                op_state
                    .resource_table
                    .take::<UserWorkerRequestResource>(rid)?,
            )
            .ok()
            .expect("multiple op_user_worker_fetch_send ongoing");

            (tx, req)
        };

        if get_upgrade_type(req.0.headers()).is_some() {
            let req_stream = state
                .borrow_mut()
                .resource_table
                .get::<HttpStreamResource>(stream_rid)?;

            let mut req_reader_mut = RcRef::map(&req_stream, |r| &r.rd).borrow_mut().await;

            if let HttpRequestReader::Headers(orig_req) = &mut *req_reader_mut {
                if let Some(upgrade) = orig_req.extensions_mut().remove::<OnUpgrade>() {
                    let _ = req.0.extensions_mut().insert(upgrade);
                }
            }
        }

        (tx, req)
    };

    let (result_tx, result_rx) = oneshot::channel::<Result<SendRequestResult, Error>>();
    let key_parsed = Uuid::try_parse(key.as_str())?;

    let watcher = watcher_rid
        .and_then(|it| {
            state
                .borrow_mut()
                .resource_table
                .take::<ConnWatcher>(it)
                .ok()
        })
        .map(Rc::try_unwrap);

    let watcher = match watcher {
        Some(Ok(it)) => it.get(),
        Some(Err(_)) => {
            error!("failed to unwrap connection watcher");
            None
        }

        None => None,
    };

    tx.send(UserWorkerMsgs::SendRequest(
        key_parsed,
        req.0,
        result_tx,
        watcher.clone(),
    ))?;

    let res = result_rx.await?;
    let (res, req_end_tx) = match res {
        Ok((res, req_end_tx)) => (res, req_end_tx),
        Err(err) => {
            error!("user worker failed to respond: {}", err);
            match err.downcast_ref() {
                Some(err @ WorkerError::RequestCancelledBySupervisor) => {
                    return Err(custom_error("WorkerRequestCancelled", err.to_string()));
                }

                None => {
                    return Err(custom_error(
                        "InvalidWorkerResponse",
                        "user worker failed to respond",
                    ));
                }
            }
        }
    };

    let mut headers = vec![];
    for (key, value) in res.headers().iter() {
        headers.push((
            ByteString::from(key.as_str()),
            ByteString::from(value.to_str().unwrap_or_default()),
        ));
    }

    let status = res.status().as_u16();
    let status_text = res
        .status()
        .canonical_reason()
        .unwrap_or("<unknown status code>")
        .to_string();

    let size = HttpBody::size_hint(res.body()).exact();
    let stream: BytesStream = Box::pin(
        res.into_body()
            .map(|r| r.map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))),
    );

    let mut op_state = state.borrow_mut();

    let body_rid = op_state.resource_table.add(UserWorkerResponseBodyResource {
        reader: AsyncRefCell::new(stream.peekable()),
        cancel: CancelHandle::default(),
        size,
        req_end_tx,
        conn_watch: watcher,
    });

    let response = UserWorkerResponse {
        status,
        status_text,
        headers,
        body_rid,
        size,
    };

    Ok(response)
}

/// Wraps a [`mpsc::Receiver`] in a [`Stream`] that can be used as a Hyper [`Body`].
pub struct BodyStream(pub mpsc::Receiver<Result<bytes::Bytes, Error>>);

impl Stream for BodyStream {
    type Item = Result<bytes::Bytes, Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.poll_recv(cx)
    }
}
