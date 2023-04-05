use std::rc::Rc;

use deno_core::error::bad_resource;
use deno_core::error::bad_resource_id;
use deno_core::error::AnyError;
use deno_core::op;
use deno_core::Extension;
use deno_core::OpState;
use deno_core::ResourceId;
use deno_http::http_create_conn_resource;
use deno_net::io::UnixStreamResource;

// TODO: Refactor into optimized exts
pub fn init() -> Extension {
    Extension::builder("http_custom")
        .ops(vec![op_http_start::decl()])
        .build()
}

// TODO: Should use optimized v8
#[op]
fn op_http_start(state: &mut OpState, stream_rid: ResourceId) -> Result<ResourceId, AnyError> {
    if let Ok(resource_rc) = state.resource_table.take::<UnixStreamResource>(stream_rid) {
        // This TCP connection might be used somewhere else. If it's the case, we cannot proceed with the
        // process of starting a HTTP server on top of this TCP connection, so we just return a bad
        // resource error. See also: https://github.com/denoland/deno/pull/16242
        let resource = Rc::try_unwrap(resource_rc)
            .map_err(|_| bad_resource("Unix stream is currently in use"))?;
        let (read_half, write_half) = resource.into_inner();
        let unix_stream = read_half.reunite(write_half)?;
        // set a hardcoded address
        let addr: std::net::SocketAddr = "0.0.0.0:9999".parse().unwrap();

        return http_create_conn_resource(state, unix_stream, addr, "http");
    }

    Err(bad_resource_id())
}
