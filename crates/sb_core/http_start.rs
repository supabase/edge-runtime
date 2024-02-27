use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::os::fd::RawFd;
use std::rc::Rc;

use deno_core::error::bad_resource;
use deno_core::error::bad_resource_id;
use deno_core::error::AnyError;
use deno_core::op2;
use deno_core::OpState;
use deno_core::ResourceId;
use deno_http::http_create_conn_resource;
use deno_net::io::UnixStreamResource;

use tokio::sync::watch;

use crate::conn_sync::ConnSync;
use crate::conn_sync::ConnWatcher;
use crate::http::UnixStream2;

#[op2]
#[serde]
fn op_http_start(
    state: &mut OpState,
    #[smi] stream_rid: ResourceId,
) -> Result<(ResourceId, ResourceId), AnyError> {
    if let Ok(resource_rc) = state.resource_table.take::<UnixStreamResource>(stream_rid) {
        // This connection might be used somewhere else. If it's the case, we cannot proceed with the
        // process of starting a HTTP server on top of this connection, so we just return a bad
        // resource error. See also: https://github.com/denoland/deno/pull/16242
        let resource = Rc::try_unwrap(resource_rc)
            .map_err(|_| bad_resource("Unix stream is currently in use"))?;

        let (read_half, write_half) = resource.into_inner();
        let unix_stream = read_half.reunite(write_half)?;
        let fd = unix_stream.as_raw_fd();
        let watcher = state
            .borrow_mut::<HashMap<RawFd, watch::Receiver<ConnSync>>>()
            .remove(&fd);

        // set a hardcoded address
        let addr: std::net::SocketAddr = "0.0.0.0:9999".parse().unwrap();
        let conn = http_create_conn_resource(
            state,
            UnixStream2::new(unix_stream, watcher.clone()),
            addr,
            "http",
        )?;

        let conn_watcher = state.resource_table.add(ConnWatcher(watcher));

        return Ok((conn, conn_watcher));
    }

    Err(bad_resource_id())
}

deno_core::extension!(sb_core_http_start, ops = [op_http_start]);
