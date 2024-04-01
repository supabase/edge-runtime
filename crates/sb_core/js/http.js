import "ext:deno_http/01_http.js";

import { core, internals, primordials } from "ext:core/mod.js";
import { fromInnerResponse, newInnerResponse } from "ext:deno_fetch/23_response.js";
import { RequestPrototype } from "ext:deno_fetch/23_request.js";
import { HttpConn, upgradeWebSocket } from "ext:sb_core_main_js/js/01_http.js";

const { internalRidSymbol } = core;
const { ObjectPrototypeIsPrototypeOf } = primordials;

const HttpConnPrototypeNextRequest = HttpConn.prototype.nextRequest;
const HttpConnPrototypeClose = HttpConn.prototype.close;
const ops = core.ops;

const kSupabaseTag = Symbol("kSupabaseTag");
const RAW_UPGRADE_RESPONSE_SENTINEL = fromInnerResponse(
	newInnerResponse(101),
	"immutable",
);

function internalServerError() {
	// "Internal Server Error"
	return new Response(
		new Uint8Array([
			73,
			110,
			116,
			101,
			114,
			110,
			97,
			108,
			32,
			83,
			101,
			114,
			118,
			101,
			114,
			32,
			69,
			114,
			114,
			111,
			114,
		]),
		{ status: 500 },
	);
}

function serveHttp(conn) {
	const [connRid, watcherRid] = ops.op_http_start(conn[internalRidSymbol]);
	const httpConn = new HttpConn(connRid, conn.remoteAddr, conn.localAddr);

	httpConn.nextRequest = async () => {
		const nextRequest = await HttpConnPrototypeNextRequest.call(httpConn);

		if (nextRequest === null) {
			return null;
		}

		nextRequest.request[kSupabaseTag] = {
			watcherRid,
			streamRid: nextRequest.streamRid
		};

		return nextRequest;
	};

	httpConn.close = () => {
		core.tryClose(watcherRid);
		HttpConnPrototypeClose.call(httpConn);
	};

	return httpConn;
}

async function serve(args1, args2) {
	let opts = {
		port: 9999,
		hostname: '0.0.0.0',
		transport: 'tcp',
	};

	const listener = Deno.listen(opts);

	if (typeof args1 === 'function') {
		opts['handler'] = args1;
	} else if (typeof args2 === 'function') {
		opts['handler'] = args2;
	} else if (typeof args1 === 'object' && typeof args1['handler'] === 'function') {
		opts['handler'] = args1['handler'];
	} else {
		throw new TypeError('A handler function must be provided.');
	}

	if (typeof args1 === 'object') {
		if (typeof args1['onListen'] === 'function') {
			opts['onListen'] = args1['onListen'];
		}
		if (typeof args1['onError'] === 'function') {
			opts['onError'] = args1['onError'];
		}
	}

	let serve;

	const handleHttp = async (conn) => {
		serve = serveHttp(conn);
		for await (const e of serve) {
			try {
				let res = await opts['handler'](e.request, {
					remoteAddr: {
						port: opts.port,
						hostname: opts.hostname,
						transport: opts.transport
					}
				});

				res = await res;

				if (res === internals.RAW_UPGRADE_RESPONSE_SENTINEL) {
					const { fenceRid } = getSupabaseTag(e.request);

					if (fenceRid === void 0) {
						throw TypeError("Cannot find a fence for upgrading response");
					}

					setTimeout(async () => {
						let {
							status,
							headers
						} = await ops.op_http_upgrade_raw2_fence(fenceRid);

						e.respondWith(new Response(null, {
							headers,
							status
						}));
					});

					continue;
				}

				e.respondWith(res);
			} catch (error) {
				if (opts['onError'] !== void 0) {
					try {
						const res = await opts['onError'](error);
						e.respondWith(res);
						continue;
					} catch(error2) {
						error = error2;
					}
				}

				console.error(error);
				e.respondWith(internalServerError());
			}
		}
	};

	const finished = (async () => {
		opts['onListen']?.({
			hostname: opts.hostname,
			port: opts.port
		});

		for await (const conn of listener) {
			handleHttp(conn);
		}
	})();

	const shutdown = () => {
		// TODO: not currently supported
	};

	return {
		finished,
		shutdown,
		ref() {
			core.refOp(serve.rid);
		},
		unref() {
			core.unrefOp(serve.rid);
		},
	};
}

function getSupabaseTag(req) {
	return req[kSupabaseTag];
}

function applySupabaseTag(src, dest) {
	if (
		!ObjectPrototypeIsPrototypeOf(RequestPrototype, src) 
		|| !ObjectPrototypeIsPrototypeOf(RequestPrototype, dest)
	) {
		throw new TypeError("Only Request instance can apply the supabase tag");
	}

	dest[kSupabaseTag] = src[kSupabaseTag];
}

internals.getSupabaseTag = getSupabaseTag;
internals.RAW_UPGRADE_RESPONSE_SENTINEL = RAW_UPGRADE_RESPONSE_SENTINEL;

export { 
	serve,
	serveHttp,
	getSupabaseTag,
	applySupabaseTag,
	upgradeWebSocket
};
