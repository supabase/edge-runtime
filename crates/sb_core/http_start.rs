use std::collections::HashMap;
use std::rc::Rc;

use deno_core::error::bad_resource;
use deno_core::error::bad_resource_id;
use deno_core::error::AnyError;
use deno_core::op2;
use deno_core::OpState;
use deno_core::ResourceId;
use deno_http::http_create_conn_resource;
use tokio::sync::watch;

use crate::conn_sync::ConnSync;
use crate::conn_sync::ConnWatcher;
use crate::http::DuplexStream2;
use crate::net::TokioDuplexResource;

#[op2]
#[serde]
fn op_http_start(
    state: &mut OpState,
    #[smi] stream_rid: ResourceId,
) -> Result<(ResourceId, ResourceId), AnyError> {
    if let Ok(resource_rc) = state.resource_table.take::<TokioDuplexResource>(stream_rid) {
        // This connection might be used somewhere else. If it's the case, we cannot proceed with the
        // process of starting a HTTP server on top of this connection, so we just return a bad
        // resource error. See also: https://github.com/denoland/deno/pull/16242
        let resource = Rc::try_unwrap(resource_rc)
            .map_err(|_| bad_resource("upstream is currently in use"))?;

        let (id, stream) = resource.into_inner();
        let watcher = state
            .borrow_mut::<HashMap<usize, watch::Receiver<ConnSync>>>()
            .remove(&id);

        // set a hardcoded address
        let addr: std::net::SocketAddr = "0.0.0.0:9999".parse().unwrap();
        let conn = http_create_conn_resource(
            state,
            DuplexStream2::new(stream, watcher.clone()),
            addr,
            "http",
        )?;

        let conn_watcher = state.resource_table.add(ConnWatcher(watcher));

        return Ok((conn, conn_watcher));
    }

    Err(bad_resource_id())
}

deno_core::extension!(sb_core_http_start, ops = [op_http_start]);
