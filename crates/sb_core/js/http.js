import { HttpConn } from "ext:deno_http/01_http.js";

const core = globalThis.Deno.core;
const ops = core.ops;

function serveHttp(conn) {
    const rid = ops.op_http_start(conn.rid);
    return new HttpConn(rid, conn.remoteAddr, conn.localAddr);
}

export { serveHttp }