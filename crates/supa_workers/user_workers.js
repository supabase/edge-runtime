const primordials = globalThis.__bootstrap.primordials;
const {
    TypeError
} = primordials;
import {
    readableStreamForRid,
    writableStreamForRid,
} from "ext:deno_web/06_streams.js";
const core = globalThis.Deno.core;
const ops = core.ops;

// interface WorkerOptions {
//     servicePath: string;
//     memoryLimitMb?: number;
//     workerTimeoutMs?: number;
//     noModuleCache?: boolean;
//     importMapPath?: string;
//     envVars?: Array<any>
// }

class UserWorker {
    constructor(key) {
        this.key = key;
    }

    async fetch(req) {
        const { method, url, headers, body, bodyUsed } = req;

        const headersArray = Array.from(headers.entries());
        const hasBody = !(method === "GET" || method === "HEAD" || bodyUsed);

        const userWorkerReq = {
            method,
            url,
            headers: headersArray,
            hasBody,
        };

        const { requestRid, requestBodyRid } = await ops.op_user_worker_fetch_build(userWorkerReq);

        // stream the request body
        if (hasBody) {
            let writableStream = writableStreamForRid(requestBodyRid);
            body.pipeTo(writableStream);
        }

        const res = await core.opAsync("op_user_worker_fetch_send", this.key, requestRid);

        const bodyStream = readableStreamForRid(res.bodyRid);
        return new Response(bodyStream, {
            headers: res.headers,
            status: res.status,
            statusText: res.statusText
        });
    }

    static async create(opts) {
        const readyOptions = {
            memory_limit_mb: 150,
            worker_timeout_ms: 60 * 1000,
            no_module_cache: false,
            import_map_path: null,
            env_vars: [],
            ...opts
        }

        const { servicePath } = readyOptions;

        if (!servicePath || servicePath === "") {
            throw new TypeError("service path must be defined");
        }

        const key = await core.opAsync("op_user_worker_create", ops);

        return new UserWorker(key);
    }
}

const SUPABASE_USER_WORKERS = UserWorker;
export { SUPABASE_USER_WORKERS };

