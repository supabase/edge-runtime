use crate::js_worker::net_override::TcpStreamResource;

use std::rc::Rc;

use deno_core::error::bad_resource;
use deno_core::error::bad_resource_id;
use deno_core::error::AnyError;
use deno_core::op;
use deno_core::Extension;
use deno_core::OpState;
use deno_core::ResourceId;
use deno_http::http_create_conn_resource;

pub fn init() -> Extension {
    Extension::builder("http_custom")
        .ops(vec![op_http_start::decl()])
        .build()
}

#[op]
fn op_http_start(state: &mut OpState, tcp_stream_rid: ResourceId) -> Result<ResourceId, AnyError> {
    if let Ok(resource_rc) = state
        .resource_table
        .take::<TcpStreamResource>(tcp_stream_rid)
    {
        // This TCP connection might be used somewhere else. If it's the case, we cannot proceed with the
        // process of starting a HTTP server on top of this TCP connection, so we just return a bad
        // resource error. See also: https://github.com/denoland/deno/pull/16242
        let resource = Rc::try_unwrap(resource_rc)
            .map_err(|_| bad_resource("TCP stream is currently in use"))?;
        let (read_half, write_half) = resource.into_inner();
        let tcp_stream = read_half.reunite(write_half)?;
        let addr = tcp_stream.local_addr()?;

        return http_create_conn_resource(state, tcp_stream, addr, "http");
    }

    Err(bad_resource_id())
}
