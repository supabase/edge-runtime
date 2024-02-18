use std::{cell::RefCell, pin::Pin, rc::Rc, task::Poll};

use deno_core::{
    error::{custom_error, AnyError},
    op2, Op, OpDecl, OpState, RcRef, ResourceId,
};
use deno_http::{HttpRequestReader, HttpStreamResource};
use deno_websocket::ws_create_server_stream;
use futures::pin_mut;
use futures::ready;
use futures::Future;
use hyper::upgrade::Parts;
use log::error;
use tokio::net::UnixStream;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::watch,
};

use crate::conn_sync::ConnSync;

deno_core::extension!(
    sb_core_http,
    ops = [op_http_upgrade_websocket2],
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

fn sb_http_middleware(decl: OpDecl) -> OpDecl {
    match decl.name {
        "op_http_upgrade_websocket" => op_http_upgrade_websocket2::DECL,
        _ => decl,
    }
}
