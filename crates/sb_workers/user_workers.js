const primordials = globalThis.__bootstrap.primordials;
const {
	TypeError,
} = primordials;
import { readableStreamForRid, writableStreamForRid } from 'ext:deno_web/06_streams.js';
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

const chunkExpression = /(?:^|\W)chunked(?:$|\W)/i;

function nullBodyStatus(status) {
	return status === 101 || status === 204 || status === 205 || status === 304;
}

function redirectStatus(status) {
	return status === 301 || status === 302 || status === 303 ||
		status === 307 || status === 308;
}

class UserWorker {
	constructor(key) {
		this.key = key;
	}

	async fetch(req, opts = {}) {
		const { method, url, headers, body, bodyUsed } = req;
		const { signal } = opts;

		signal?.throwIfAborted();

		const headersArray = Array.from(headers.entries());
		const hasReqBody = !bodyUsed && !!body &&
			(chunkExpression.test(headers.get('transfer-encoding')) ||
				Number.parseInt(headers.get('content-length'), 10) > 0);

		const userWorkerReq = {
			method,
			url,
			headers: headersArray,
			hasBody: hasReqBody,
		};

		const { requestRid, requestBodyRid } = await ops.op_user_worker_fetch_build(
			userWorkerReq,
		);

		// stream the request body
		let reqBodyPromise = null;
		if (hasReqBody) {
			let writableStream = writableStreamForRid(requestBodyRid);
			reqBodyPromise = body.pipeTo(writableStream, { signal });
		}

		const resPromise = core.opAsync('op_user_worker_fetch_send', this.key, requestRid);
		const [, res] = await Promise.all([reqBodyPromise, resPromise]);

		const response = {
			headers: res.headers,
			status: res.status,
			statusText: res.statusText,
			body: null,
		};

		// TODO: add a test
		if (nullBodyStatus(res.status) || redirectStatus(res.status)) {
			core.close(res.bodyRid);
		} else {
			if (req.method === 'HEAD' || req.method === 'CONNECT') {
				response.body = null;
				core.close(res.bodyRid);
			} else {
				const bodyStream = readableStreamForRid(res.bodyRid);

				signal?.addEventListener('abort', () => {
					core.tryClose(res.bodyRid);
				});
				response.body = bodyStream;
			}
		}

		return new Response(response.body ? response.body : null, {
			headers: response.headers,
			status: response.status,
			statusText: response.statusText,
		});
	}

	static async create(opts) {
		const readyOptions = {
			memoryLimitMb: 512,
			lowMemoryMultiplier: 5,
			workerTimeoutMs: 5 * 60 * 1000,
			cpuTimeThresholdMs: 50,
			cpuBurstIntervalMs: 100,
			maxCpuBursts: 10,
			noModuleCache: false,
			importMapPath: null,
			envVars: [],
			forceCreate: false,
			netAccessDisabled: false,
			customModuleRoot: '',
			maybeEszip: null,
			maybeEntrypoint: null,
			...opts,
		};

		const { servicePath, maybeEszip } = readyOptions;

		if (!maybeEszip && (!servicePath || servicePath === '')) {
			throw new TypeError('service path must be defined');
		}

		const key = await core.opAsync('op_user_worker_create', readyOptions);

		return new UserWorker(key);
	}
}

const SUPABASE_USER_WORKERS = UserWorker;
export { SUPABASE_USER_WORKERS };
