import { primordials, core } from "ext:core/mod.js";
import { readableStreamForRid, writableStreamForRid } from 'ext:deno_web/06_streams.js';
import { getSupabaseTag } from 'ext:sb_core_main_js/js/http.js';

const ops = core.ops;
const {
	InterruptedPrototype,
} = core;
const { 
	TypeError,
	ObjectPrototypeIsPrototypeOf,
	StringPrototypeIncludes,
} = primordials;
const {
	op_user_worker_fetch_send,
	op_user_worker_create,
} = ops;

const NO_SUPABASE_TAG_WARN_MSG = `Unable to find the supabase tag from the request instance.\n\
Invoke \`EdgeRuntime.applySupabaseTag(origReq, newReq)\` if you have cloned the original request.`

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
		const tag = getSupabaseTag(req);
		
		const { method, url, headers, body, bodyUsed } = req;
		const { signal } = opts;

		signal?.throwIfAborted();

		if (tag === void 0) {
			console.warn(NO_SUPABASE_TAG_WARN_MSG);
		} 

		const headersArray = Array.from(headers.entries());
		const hasBody = !bodyUsed && !!body;

		const userWorkerReq = {
			method,
			url,
			hasBody,
			headers: headersArray,
		};

		const { requestRid, requestBodyRid } = await ops.op_user_worker_fetch_build(
			userWorkerReq,
		);

		// stream the request body
		let reqBodyPromise = null;
		if (hasBody) {
			let writableStream = writableStreamForRid(requestBodyRid);
			reqBodyPromise = body.pipeTo(writableStream, { signal });
		}

		const resPromise = op_user_worker_fetch_send(
			this.key,
			requestRid,
			requestBodyRid,
			tag.streamRid,
			tag.watcherRid
		);

		let [sent, res] = await Promise.allSettled([reqBodyPromise, resPromise]);
		
		if (sent.status === "rejected") {
			if (res.status === "fulfilled") {
				res = res.value;
			} else {
				if (
					ObjectPrototypeIsPrototypeOf(InterruptedPrototype, sent.reason) ||
					StringPrototypeIncludes(sent.reason.message, "operation canceled")
				) {
					throw res.reason;
				} else {
					throw sent.reason;
				}
			}
		} else if (res.status === "rejected") {
			throw res.reason;
		} else {
			res = res.value;
		}

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
			cpuTimeSoftLimitMs: 50,
			cpuTimeHardLimitMs: 100,
			noModuleCache: false,
			importMapPath: null,
			envVars: [],
			forceCreate: false,
			netAccessDisabled: false,
			allowRemoteModules: true,
			customModuleRoot: '',
			maybeEszip: null,
			maybeEntrypoint: null,
			maybeModuleCode: null,
			...opts,
		};

		const { servicePath, maybeEszip } = readyOptions;

		if (!maybeEszip && (!servicePath || servicePath === '')) {
			throw new TypeError('service path must be defined');
		}

		const key = await op_user_worker_create(readyOptions);

		return new UserWorker(key);
	}
}

const SUPABASE_USER_WORKERS = UserWorker;
export { SUPABASE_USER_WORKERS };
