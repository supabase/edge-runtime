import { primordials, core } from "ext:core/mod.js";
import { SymbolDispose } from "ext:deno_web/00_infra.js";
import { readableStreamForRid, writableStreamForRid } from "ext:deno_web/06_streams.js";
import { getSupabaseTag } from "ext:sb_core_main_js/js/http.js";

const ops = core.ops;

const { TypeError } = primordials;

const {
	op_user_worker_fetch_send,
	op_user_worker_create,
	op_user_user_worker_wait_token_cancelled,
	op_user_worker_is_active,
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
	/** @type {string} */
	#key = "";

	/** @type {number | null} */
	#rid = null;

	/** @type {boolean} */
	#disposed = false;

	/** 
	 * @param {string} key
	 * @param {number} rid
	 */
	constructor(key, rid) {
		this.#key = key;
		this.#rid = rid;

		// deno-lint-ignore no-this-alias
		const self = this;

		setTimeout(async () => {
			try {
				await op_user_user_worker_wait_token_cancelled(rid);
				self.dispose();
			} catch {
				// TODO(Nyannyacha): Link it with the tracing for telemetry.
			}
		});
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
			requestBodyPromise = body.pipeTo(writableStreamForRid(requestBodyRid), { signal });
		}

		const responsePromise = op_user_worker_fetch_send(
			this.#key,
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
			// TODO(Nyannyacha): Link it with the tracing for telemetry.
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

	/** @returns {boolean} */
	get active() {
		if (this.#disposed) {
			return false;
		}

		return op_user_worker_is_active(this.#rid);
	}

	dispose() {
		if (!this.#disposed) {
			core.tryClose(this.#rid);
			this.#disposed = true;
		}
	}

	[SymbolDispose]() {
		this.dispose();
	}

	static async create(opts) {
		const readyOptions = {
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

		const [key, rid] = await op_user_worker_create(readyOptions);

		return new UserWorker(key, rid);
	}
}

const SUPABASE_USER_WORKERS = UserWorker;

export { SUPABASE_USER_WORKERS };