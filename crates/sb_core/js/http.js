import { HttpConn } from 'ext:deno_http/01_http.js';
import { RequestPrototype } from 'ext:deno_fetch/23_request.js';
import { core, primordials } from "ext:core/mod.js";
const { ObjectPrototypeIsPrototypeOf } = primordials;

const HttpConnPrototypeNextRequest = HttpConn.prototype.nextRequest;
const HttpConnPrototypeClose = HttpConn.prototype.close;
const ops = core.ops;

const watcher = Symbol("watcher");

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
	const [connRid, watcherRid] = ops.op_http_start(conn.rid);
	const httpConn = new HttpConn(connRid, conn.remoteAddr, conn.localAddr);

	httpConn.nextRequest = async () => {
		const nextRequest = await HttpConnPrototypeNextRequest.call(httpConn);

		if (nextRequest === null) {
			return null;
		}

		nextRequest.request[watcher] = watcherRid;

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
	} else if (typeof args1 === 'object' && typeof args2 === 'function') {
		opts['handler'] = args2;
	} else if (typeof args1 === 'object') {
		if (typeof args1['handler'] === 'function') {
			opts['handler'] = args1['handler'];
		}
		if (typeof args1['onListen'] === 'function') {
			opts['onListen'] = args1['onListen'];
		}
	} else {
		throw new TypeError('A handler function must be provided.');
	}

	let serve;

	const handleHttp = async (conn) => {
		serve = serveHttp(conn);
		for await (const e of serve) {
			try {
				const res = await opts['handler'](e.request, {
					remoteAddr: {
						port: opts.port,
						hostname: opts.hostname,
						transport: opts.transport
					}
				});

				e.respondWith(res);
			} catch (error) {
				console.error(error);
				return e.respondWith(internalServerError());
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

function getWatcherRid(req) {
	return req[watcher];
}

function applyWatcherRid(src, dest) {
	if (
		!ObjectPrototypeIsPrototypeOf(RequestPrototype, src) 
		|| !ObjectPrototypeIsPrototypeOf(RequestPrototype, dest)
	) {
		throw new TypeError("Only Request instance can apply the connection watcher");
	}

	dest[watcher] = src[watcher];
}

export { serve, serveHttp, getWatcherRid, applyWatcherRid };
