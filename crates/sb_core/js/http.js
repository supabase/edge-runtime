import { HttpConn } from "ext:deno_http/01_http.js";

const core = globalThis.Deno.core;
const ops = core.ops;

function serveHttp(conn) {
    const rid = ops.op_http_start(conn.rid);
    return new HttpConn(rid, conn.remoteAddr, conn.localAddr);
}

async function serve(args1, args2) {
    let opts = {
        port: 9999,
        hostname: "0.0.0.0",
        transport: "tcp"
    }

    const listener = Deno.listen(opts);

    if (typeof args1 === 'function') {
        opts['handler'] = args1;
    } else if (typeof args1 === 'object' && typeof args2 === 'function') {
        opts['handler'] = args2;
    } else {
        if (typeof handler !== "function") {
            throw new TypeError("A handler function must be provided.");
        }
    }

    let serve;

    const handleHttp = async (conn) => {
        serve = serveHttp(conn);
        for await (const e of serve) {
            const res = await opts['handler'](e.request);
            e.respondWith(res);
        }
    }

    for await (const conn of listener) {
        handleHttp(conn);
    }

    return {
        finished: async () => {},
        ref() {
            core.refOp(serve.rid);
        },
        unref() {
            core.unrefOp(serve.rid);
        }
    }
}



export { serveHttp, serve }