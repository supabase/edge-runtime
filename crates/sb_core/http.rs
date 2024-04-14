use std::{borrow::Cow, cell::RefCell, pin::Pin, rc::Rc, task::Poll};

use anyhow::{bail, Context};
use deno_core::{
    error::{custom_error, AnyError},
    op2, ByteString, Op, OpDecl, OpState, RcRef, Resource, ResourceId,
};
use deno_http::{HttpRequestReader, HttpStreamResource};
use deno_websocket::ws_create_server_stream;
use futures::pin_mut;
use futures::ready;
use futures::Future;
use hyper::upgrade::{OnUpgrade, Parts};
use log::error;
use serde::Serialize;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{oneshot, watch},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};

use crate::{
    conn_sync::ConnSync,
    upgrade::{UpgradeStream, WebSocketUpgrade},
};

deno_core::extension!(
    sb_core_http,
    ops = [
        op_http_upgrade_websocket2,
        op_http_upgrade_raw2,
        op_http_upgrade_raw2_fence
    ],
    middleware = sb_http_middleware,
);

pub(crate) struct UnixStream2(UnixStream, Option<watch::Receiver<ConnSync>>);

impl UnixStream2 {
    pub fn new(stream: UnixStream, watcher: Option<watch::Receiver<ConnSync>>) -> Self {
        Self(stream, watcher)
    }
}

impl AsyncRead for UnixStream2 {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut Pin::into_inner(self).0).poll_read(cx, buf)
    }
}

impl AsyncWrite for UnixStream2 {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut Pin::into_inner(self).0).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut Pin::into_inner(self).0).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    #[inline]
    fn poll_flush(
        self: Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        if let Some(ref mut sync) = self.1 {
            let fut = sync.wait_for(|it| *it == ConnSync::Recv);

            pin_mut!(fut);
            ready!(fut.poll(cx).map(|it| {
                if let Err(ex) = it {
                    error!("cannot track outbound connection correctly");
                    return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, ex));
                }

                Ok(())
            }))?
        }

        Pin::new(&mut Pin::into_inner(self).0).poll_shutdown(cx)
    }
}

fn http_error(message: &'static str) -> AnyError {
    custom_error("Http", message)
}

#[op2(async)]
#[smi]
async fn op_http_upgrade_websocket2(
    state: Rc<RefCell<OpState>>,
    #[smi] rid: ResourceId,
) -> Result<ResourceId, AnyError> {
    let stream = state
        .borrow_mut()
        .resource_table
        .get::<HttpStreamResource>(rid)?;

    let mut rd = RcRef::map(&stream, |r| &r.rd).borrow_mut().await;

    let request = match &mut *rd {
        HttpRequestReader::Headers(request) => request,
        _ => return Err(http_error("cannot upgrade because request body was used")),
    };

    let upgraded = hyper::upgrade::on(request).await?;
    let Parts { io, read_buf, .. } = upgraded.downcast::<UnixStream2>().unwrap();

    let ws_rid = ws_create_server_stream(&mut state.borrow_mut(), io.0.into(), read_buf)?;
    Ok(ws_rid)
}

#[op2]
#[serde]
fn op_http_upgrade_raw2(
    state: &mut OpState,
    #[smi] stream_rid: ResourceId,
) -> Result<(ResourceId, ResourceId), AnyError> {
    let req_stream = state.resource_table.get::<HttpStreamResource>(stream_rid)?;
    let mut req_reader_mut = RcRef::map(&req_stream, |r| &r.rd)
        .try_borrow_mut()
        .with_context(|| "unable to get http stream reader")?;

    // Stage 1: extract the upgrade future
    let upgrade = if let HttpRequestReader::Headers(orig_req) = &mut *req_reader_mut {
        orig_req.extensions_mut().remove::<OnUpgrade>()
    } else {
        None
    }
    .with_context(|| "no upgrade extension was found")?;

    drop(req_reader_mut);

    let (resp_tx, resp_rx) = oneshot::channel();
    let (read, write) = tokio::io::duplex(1024);
    let (read_rx, write_tx) = tokio::io::split(read);
    let (mut write_rx, mut read_tx) = tokio::io::split(write);

    tokio::spawn(async move {
        let mut upgrade_stream = WebSocketUpgrade::<()>::default();

        // Stage 2: Extract the Upgraded connection
        let mut buf = [0; 1024];
        let upgraded = loop {
            let read = Pin::new(&mut write_rx).read(&mut buf).await?;
            match upgrade_stream.write(&buf[..read]) {
                Ok(None) => continue,
                Ok(Some((resp, pre))) => {
                    if resp_tx.send(resp).is_err() {
                        bail!("cannot send response");
                    }

                    let mut upgraded = upgrade.await?;

                    upgraded.write_all(&pre).await?;
                    break upgraded;
                }

                Err(err) => return Err(err),
            }
        };

        // Stage 3: Pump the data
        let (mut upgraded_rx, mut upgraded_tx) = tokio::io::split(upgraded);

        tokio::spawn(async move {
            let mut buf = [0; 1024];
            loop {
                let read = upgraded_rx.read(&mut buf).await?;
                if read == 0 {
                    break;
                }
                read_tx.write_all(&buf[..read]).await?;
            }

            read_tx.shutdown().await
        });

        tokio::spawn(async move {
            let mut buf = [0; 1024];
            loop {
                let read = write_rx.read(&mut buf).await?;
                if read == 0 {
                    break;
                }
                upgraded_tx.write_all(&buf[..read]).await?;
            }

            upgraded_tx.shutdown().await
        });

        Ok::<_, AnyError>(())
    });

    Ok((
        state
            .resource_table
            .add(UpgradeStream::new(read_rx, write_tx)),
        state
            .resource_table
            .add(HttpUpgradeRawResponseFenceResource(resp_rx)),
    ))
}

#[op2(async)]
#[serde]
async fn op_http_upgrade_raw2_fence(
    state: Rc<RefCell<OpState>>,
    #[smi] rid: ResourceId,
) -> Result<HttpUpgradeRawResponseResource, AnyError> {
    let resp = state
        .borrow_mut()
        .resource_table
        .take::<HttpUpgradeRawResponseFenceResource>(rid)?;

    Ok(HttpUpgradeRawResponseResource::new(
        Rc::into_inner(resp)
            .unwrap()
            .0
            .await
            .with_context(|| "cannot receive response")?,
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HttpUpgradeRawResponseResource {
    status: u16,
    headers: Vec<(ByteString, ByteString)>,
}

impl Resource for HttpUpgradeRawResponseResource {
    fn name(&self) -> Cow<str> {
        "httpUpgradeRawResponseResource".into()
    }
}

impl HttpUpgradeRawResponseResource {
    fn new(res: http::Response<()>) -> Self {
        let status = res.status().as_u16();
        let mut headers = vec![];

        for (key, value) in res.headers().iter() {
            headers.push((
                ByteString::from(key.as_str()),
                ByteString::from(value.to_str().unwrap_or_default()),
            ));
        }

        Self { status, headers }
    }
}

struct HttpUpgradeRawResponseFenceResource(oneshot::Receiver<http::Response<()>>);

impl Resource for HttpUpgradeRawResponseFenceResource {
    fn name(&self) -> Cow<str> {
        "httpUpgradeRawResponseFenceResource".into()
    }
}

fn sb_http_middleware(decl: OpDecl) -> OpDecl {
    match decl.name {
        "op_http_upgrade_websocket" => op_http_upgrade_websocket2::DECL,
        _ => decl,
    }
}
