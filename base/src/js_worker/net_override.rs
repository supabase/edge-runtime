use anyhow::Error;
use deno_core::error::bad_resource;
use deno_core::error::AnyError;
use deno_core::op;
use deno_core::AsyncRefCell;
use deno_core::AsyncResult;
use deno_core::CancelHandle;
use deno_core::CancelTryFuture;
use deno_core::Extension;
use deno_core::OpState;
use deno_core::RcRef;
use deno_core::Resource;
use deno_core::ResourceId;
use deno_net::ops::IpAddr;
use std::cell::RefCell;
use std::rc::Rc;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

pub struct TcpStreamResource {
    rd: AsyncRefCell<tokio::net::tcp::OwnedReadHalf>,
    wr: AsyncRefCell<tokio::net::tcp::OwnedWriteHalf>,
    // When a `TcpStream` resource is closed, all pending 'read' ops are
    // canceled, while 'write' ops are allowed to complete. Therefore only
    // 'read' futures are attached to this cancel handle.
    cancel: CancelHandle,
}

impl TcpStreamResource {
    pub fn into_inner(
        self,
    ) -> (
        tokio::net::tcp::OwnedReadHalf,
        tokio::net::tcp::OwnedWriteHalf,
    ) {
        (self.rd.into_inner(), self.wr.into_inner())
    }

    async fn read(self: Rc<Self>, data: &mut [u8]) -> Result<usize, Error> {
        let mut rd = RcRef::map(&self, |r| &r.rd).borrow_mut().await;
        let cancel = RcRef::map(self, |r| &r.cancel);
        let nread = rd.read(data).try_or_cancel(cancel).await?;
        Ok(nread)
    }

    async fn write(self: Rc<Self>, data: &[u8]) -> Result<usize, Error> {
        let mut wr = RcRef::map(self, |r| &r.wr).borrow_mut().await;
        let nwritten = wr.write(data).await?;
        Ok(nwritten)
    }
}

impl Resource for TcpStreamResource {
    deno_core::impl_readable_byob!();
    deno_core::impl_writable!();

    fn close(self: Rc<Self>) {
        self.cancel.cancel()
    }
}

impl From<tokio::net::TcpStream> for TcpStreamResource {
    fn from(s: tokio::net::TcpStream) -> Self {
        let (rd, wr) = s.into_split();
        Self {
            rd: rd.into(),
            wr: wr.into(),
            cancel: Default::default(),
        }
    }
}

#[op]
fn op_net_listen(_state: &mut OpState) -> Result<(ResourceId, IpAddr), AnyError> {
    // this is a noop
    // TODO: customize to match the service ip and port
    Ok((
        0,
        IpAddr {
            hostname: "0.0.0.0".to_string(),
            port: 9999,
        },
    ))
}

#[op]
async fn op_net_accept(
    state: Rc<RefCell<OpState>>,
) -> Result<(ResourceId, IpAddr, IpAddr), AnyError> {
    let mut op_state = state.borrow_mut();

    // we do not want to keep the op_state locked,
    // so we take the channel receiver from it and release op state.
    // we need to add it back later after processing a message.
    let mut rx = op_state.take::<mpsc::UnboundedReceiver<tokio::net::TcpStream>>();
    drop(op_state);

    let tcp_stream = rx.recv().await;
    if tcp_stream.is_none() {
        println!("no tcp stream found");
        return Err(bad_resource("channel is closed"));
    }
    let tcp_stream = tcp_stream.unwrap();

    let local_addr = IpAddr::from(tcp_stream.local_addr().unwrap().clone());
    let remote_addr = IpAddr::from(tcp_stream.peer_addr().unwrap().clone());

    let tcp_stream_resource = TcpStreamResource::from(tcp_stream);

    // since the op state was dropped before,
    // reborrow and add the channel receiver again
    let mut op_state = state.borrow_mut();
    op_state.put::<mpsc::UnboundedReceiver<tokio::net::TcpStream>>(rx);
    let rid = op_state.resource_table.add(tcp_stream_resource);
    Ok((rid, local_addr, remote_addr))
}

pub fn init() -> Extension {
    Extension::builder("net_override")
        .middleware(|op| match op.name {
            "op_net_listen_tcp" => op_net_listen::decl(),
            "op_net_accept_tcp" => op_net_accept::decl(),
            _ => op,
        })
        .build()
}
