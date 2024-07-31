import { primordials, core } from "ext:core/mod.js";
import { readableStreamForRid, writableStreamForRid } from "ext:deno_web/06_streams.js";
import { getSupabaseTag } from "ext:sb_core_main_js/js/http.js";

const ops = core.ops;

const { TypeError } = primordials;

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

	async fetch(request, options = {}) {
		const tag = getSupabaseTag(request);
		
		const { method, url, headers, body, bodyUsed } = request;
		const { signal } = options;

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
		let requestBodyPromise = null;

		if (hasBody) {
			let writableStream = writableStreamForRid(requestBodyRid);
			requestBodyPromise = body.pipeTo(writableStream, { signal });
		}

		const responsePromise = op_user_worker_fetch_send(
			this.key,
			requestRid,
			requestBodyRid,
			tag.streamRid,
			tag.watcherRid
		);

		const [requestBodyPromiseResult, responsePromiseResult] = await Promise.allSettled([
			requestBodyPromise,
			responsePromise
		]);

		if (requestBodyPromiseResult.status === "rejected") {
			// console.warn(requestBodyPromiseResult.reason);
		}

		if (responsePromiseResult.status === "rejected") {
			throw responsePromiseResult.reason;
		}

		const result = responsePromiseResult.value;
		const response = {
			headers: result.headers,
			status: result.status,
			statusText: result.statusText,
			body: null,
		};

		// TODO: add a test
		if (nullBodyStatus(result.status) || redirectStatus(result.status)) {
			core.tryClose(result.bodyRid);
		} else {
			if (request.method === "HEAD" || request.method === "CONNECT") {
				core.tryClose(result.bodyRid);
			} else {
				const stream = readableStreamForRid(result.bodyRid);

				signal?.addEventListener("abort", () => {
					core.tryClose(result.bodyRid);
				});
				
				response.body = stream;
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
			allowNet: null,
			allowRemoteModules: true,
			customModuleRoot: '',
			maybeEszip: null,
			maybeEntrypoint: null,
			maybeModuleCode: null,
			...opts,
		};

		const { servicePath, maybeEszip } = readyOptions;

		if (!maybeEszip && (!servicePath || servicePath === "")) {
			throw new TypeError("service path must be defined");
		}

		const key = await op_user_worker_create(readyOptions);

		return new UserWorker(key);
	}
}

const SUPABASE_USER_WORKERS = UserWorker;

export { SUPABASE_USER_WORKERS };
