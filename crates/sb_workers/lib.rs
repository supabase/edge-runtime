pub mod context;

use crate::context::{
    CreateUserWorkerResult, UserWorkerMsgs, UserWorkerRuntimeOpts, WorkerContextInitOpts,
    WorkerRuntimeOpts,
};
use anyhow::Error;
use context::SendRequestResult;
use deno_core::error::{custom_error, type_error, AnyError};
use deno_core::futures::stream::Peekable;
use deno_core::futures::{Stream, StreamExt};
use deno_core::op2;
use deno_core::{
    AsyncRefCell, AsyncResult, BufView, ByteString, CancelFuture, CancelHandle, CancelTryFuture,
    JsBuffer, OpState, RcRef, Resource, ResourceId, WriteOutcome,
};
use hyper::body::HttpBody;
use hyper::header::{HeaderName, HeaderValue};
use hyper::{Body, Request};
use sb_graph::EszipPayloadKind;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};
use tokio::sync::{mpsc, oneshot};
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
            timing_rx_pair: None,
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
    method: String,
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
    body: AsyncRefCell<mpsc::Sender<Option<bytes::Bytes>>>,
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
            let cancel = RcRef::map(self, |r| &r.cancel);
            body.send(Some(bytes))
                .or_cancel(cancel)
                .await?
                .map_err(|_| type_error("request body receiver not connected (request closed)"))?;
            Ok(WriteOutcome::Full { nwritten })
        })
    }

    fn shutdown(self: Rc<Self>) -> AsyncResult<()> {
        Box::pin(async move {
            let body = RcRef::map(&self, |r| &r.body).borrow_mut().await;
            let cancel = RcRef::map(self, |r| &r.cancel);
            // There is a case where hyper knows the size of the response body up
            // front (through content-length header on the resp), where it will drop
            // the body once that content length has been reached, regardless of if
            // the stream is complete or not. This is expected behaviour, but it means
            // that if you stream a body with an up front known size (eg a Blob),
            // explicit shutdown can never succeed because the body (and by extension
            // the receiver) will have dropped by the time we try to shutdown. As such
            // we ignore if the receiver is closed, because we know that the request
            // is complete in good health in that case.
            body.send(None).or_cancel(cancel).await?.ok();
            Ok(())
        })
    }

    fn close(self: Rc<Self>) {
        self.cancel.cancel()
    }
}

type BytesStream = Pin<Box<dyn Stream<Item = Result<bytes::Bytes, std::io::Error>> + Unpin>>;

struct UserWorkerResponseBodyResource {
    reader: AsyncRefCell<Peekable<BytesStream>>,
    cancel: CancelHandle,
    size: Option<u64>,
    end_req_tx: mpsc::UnboundedSender<()>,
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
        let _ = self.end_req_tx.send(());
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
    let mut body = Body::empty();
    let mut request_body_rid = None;
    if req.has_body {
        let (stream, tx) = MpscByteStream::new();
        body = Body::wrap_stream(stream);

        request_body_rid = Some(state.resource_table.add(UserWorkerRequestBodyResource {
            body: AsyncRefCell::new(tx),
            cancel: CancelHandle::default(),
        }));
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
            let mut header_value =
                HeaderValue::try_from(value).unwrap_or(HeaderValue::from_static(""));

            // if request has no body explicitly set the content-length to 0
            if !req.has_body && header_name.eq("content-length") {
                header_value = HeaderValue::from(0);
            }

            request.headers_mut().append(header_name, header_value);
        }
    }

    let request_rid = state.resource_table.add(UserWorkerRequestResource(request));

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
) -> Result<UserWorkerResponse, AnyError> {
    let (tx, request) = {
        let mut op_state = state.borrow_mut();
        let tx = op_state
            .borrow::<mpsc::UnboundedSender<UserWorkerMsgs>>()
            .clone();

        let request = op_state
            .resource_table
            .take::<UserWorkerRequestResource>(rid)?;

        (tx, request)
    };

    let request = Rc::try_unwrap(request)
        .ok()
        .expect("multiple op_user_worker_fetch_send ongoing");
    let (result_tx, result_rx) = oneshot::channel::<Result<SendRequestResult, Error>>();
    let key_parsed = Uuid::try_parse(key.as_str())?;
    tx.send(UserWorkerMsgs::SendRequest(
        key_parsed, request.0, result_tx,
    ))?;

    let result = result_rx.await?;
    if result.is_err() {
        return Err(custom_error(
            "InvalidWorkerResponse",
            "user worker failed to respond",
        ));
    }

    let (result, end_req_tx) = result.unwrap();

    let mut headers = vec![];
    for (key, value) in result.headers().iter() {
        headers.push((
            ByteString::from(key.as_str()),
            ByteString::from(value.to_str().unwrap_or_default()),
        ));
    }

    let status = result.status().as_u16();
    let status_text = result
        .status()
        .canonical_reason()
        .unwrap_or("<unknown status code>")
        .to_string();

    let size = HttpBody::size_hint(result.body()).exact();
    let stream: BytesStream = Box::pin(
        result
            .into_body()
            .map(|r| r.map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))),
    );

    let mut op_state = state.borrow_mut();
    let body_rid = op_state.resource_table.add(UserWorkerResponseBodyResource {
        reader: AsyncRefCell::new(stream.peekable()),
        cancel: CancelHandle::default(),
        size,
        end_req_tx,
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

// [copied from https://github.com/denoland/deno/blob/v1.31.3/ext/fetch/byte_stream.rs]
// [MpscByteStream] is a stream of bytes that is backed by a mpsc channel. It is
// used to bridge between the fetch task and the HTTP body stream. The stream
// has the special property that it errors if the channel is closed before an
// explicit EOF is sent (in the form of a [None] value on the sender).
pub struct MpscByteStream {
    receiver: mpsc::Receiver<Option<bytes::Bytes>>,
    shutdown: bool,
}

impl MpscByteStream {
    pub fn new() -> (Self, mpsc::Sender<Option<bytes::Bytes>>) {
        let (sender, receiver) = mpsc::channel::<Option<bytes::Bytes>>(1);
        let this = Self {
            receiver,
            shutdown: false,
        };
        (this, sender)
    }
}

impl Stream for MpscByteStream {
    type Item = Result<bytes::Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let val = std::task::ready!(self.receiver.poll_recv(cx));
        match val {
            None if self.shutdown => Poll::Ready(None),
            // handle chunked readable streams
            None => {
                self.shutdown = true;
                Poll::Ready(None)
            }
            Some(None) => {
                self.shutdown = true;
                Poll::Ready(None)
            }
            Some(Some(val)) => Poll::Ready(Some(Ok(val))),
        }
    }
}
