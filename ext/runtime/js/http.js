import "ext:deno_http/01_http.js";

import { core, internals, primordials } from "ext:core/mod.js";
import { enterRequestContext } from "ext:runtime/request_context.js";
import { RequestPrototype } from "ext:deno_fetch/23_request.js";
import {
  fromInnerResponse,
  newInnerResponse,
  ResponsePrototype,
} from "ext:deno_fetch/23_response.js";
import { upgradeWebSocket } from "ext:deno_http/02_websocket.ts";
import { HttpConn } from "ext:runtime/01_http.js";
import { performance } from "ext:deno_web/15_performance.js";
import {
  builtinTracer,
  ContextManager,
  currentSnapshot,
  enterSpan,
  PROPAGATORS,
  restoreSnapshot,
  TRACING_ENABLED,
} from "ext:deno_telemetry/telemetry.ts";
import {
  updateSpanFromRequest,
  updateSpanFromResponse,
} from "ext:deno_telemetry/util.ts";

const ops = core.ops;

const {
  BadResourcePrototype,
  internalRidSymbol,
  setAsyncContext,
} = core;
const {
  ArrayPrototypeFind,
  ArrayPrototypeMap,
  ArrayPrototypePush,
  DateNow,
  MathAbs,
  MathRound,
  NumberParseInt,
  RegExpPrototypeExec,
  SafeArrayIterator,
  ObjectPrototypeIsPrototypeOf,
  SafePromisePrototypeFinally,
  String,
} = primordials;

const HttpConnPrototypeNextRequest = HttpConn.prototype.nextRequest;
const HttpConnPrototypeClose = HttpConn.prototype.close;

const kSupabaseTag = Symbol("kSupabaseTag");

let ACTIVE_REQUESTS = 0;

const HTTP_CONNS = new Set();
const RAW_UPGRADE_RESPONSE_SENTINEL = fromInnerResponse(
  newInnerResponse(101),
  "immutable",
);

function internalServerError() {
  return new Response("Internal Server Error", { status: 500 });
}

function badGatewayError() {
  return new Response("Bad Gateway", { status: 502 });
}

// When the platform's front door received the request: `t=<microseconds since the Unix epoch>`.
const REQUEST_START_HEADER = "x-request-start";
const REQUEST_START_PATTERN = /^t=(\d{16})$/;
// How long the request took from the front door to here, in microseconds. Set on a user
// worker's response for the main worker, which records it and strips it.
const START_LATENCY_HEADER = "sb-start-latency-us";

/**
 * Microseconds from the front door's `x-request-start` until now, or null when the request
 * carries no stamp. Read when the request is handed to the function's code, so it covers
 * everything the platform did before that: routing, fetching the bundle, booting the worker
 * and evaluating the module.
 */
function startLatencyUs(request) {
  const stamp = request.headers.get(REQUEST_START_HEADER);
  if (stamp === null) {
    return null;
  }
  const match = RegExpPrototypeExec(REQUEST_START_PATTERN, stamp);
  if (match === null) {
    return null;
  }
  return MathRound(wallClockMs() * 1000 - NumberParseInt(match[1], 10));
}

/**
 * Wall-clock time in milliseconds, which is what the front door stamps. DateNow has only
 * millisecond resolution, so this uses the high-resolution clock while it agrees with
 * DateNow, which also bounds what a function patching `performance` can do to the result.
 */
function wallClockMs() {
  const coarse = DateNow();
  const fine = performance.timeOrigin + performance.now();
  return MathAbs(fine - coarse) < 2 ? fine : coarse;
}

/**
 * Sets the start latency on a user worker's response, replacing any value the function set,
 * or removes the header when the request carried no stamp. Either way the main worker only
 * ever sees a value the runtime measured.
 */
function withStartLatency(response, latencyUs) {
  // A 101 carries the upgraded connection; copying it would lose that.
  if (response.status === 101) {
    return response;
  }
  try {
    setStartLatency(response.headers, latencyUs);
    return response;
  } catch {
    // Immutable headers, as on a fetch() response passed straight through.
    const copy = new Response(response.body, response);
    setStartLatency(copy.headers, latencyUs);
    return copy;
  }
}

function setStartLatency(headers, latencyUs) {
  if (latencyUs === null) {
    headers.delete(START_LATENCY_HEADER);
  } else {
    headers.set(START_LATENCY_HEADER, String(latencyUs));
  }
}

function serveHttp(conn) {
  let closed = false;

  const [connRid, watcherRid] = ops.op_http_start(conn[internalRidSymbol]);
  const httpConn = new HttpConn(connRid, conn.remoteAddr, conn.localAddr);

  httpConn.nextRequest = async () => {
    const nextRequest = await HttpConnPrototypeNextRequest.call(httpConn);

    if (nextRequest === null) {
      return null;
    }

    nextRequest.request[kSupabaseTag] = {
      watcherRid,
      streamRid: nextRequest.streamRid,
    };

    const isUserWorker = internals.worker?.kind === "user";
    const latencyUs = isUserWorker ? startLatencyUs(nextRequest.request) : null;

    return {
      ...nextRequest,
      respondWith: async function (resp) {
        let needThrow = true;
        try {
          resp = await resp;
          if (!(ObjectPrototypeIsPrototypeOf(ResponsePrototype, resp))) {
            needThrow = false;
            await nextRequest.respondWith(badGatewayError());
            throw new TypeError(
              "First argument to 'respondWith' must be a Response or a promise resolving to a Response",
            );
          }
          if (isUserWorker) {
            resp = withStartLatency(resp, latencyUs);
          }
          return await nextRequest.respondWith(resp);
        } catch (error) {
          console.error(error);
          if (needThrow) {
            throw error;
          }
        }
      },
    };
  };

  httpConn.close = () => {
    if (!closed) {
      closed = true;
      HTTP_CONNS.delete(httpConn);
      core.tryClose(watcherRid);
      HttpConnPrototypeClose.call(httpConn);
    }
  };

  HTTP_CONNS.add(httpConn);

  return httpConn;
}

function serve(args1, args2) {
  const options = {
    port: 9999,
    hostname: "0.0.0.0",
    transport: "tcp",
  };

  const listener = Deno.listen(options);
  const snapshot = currentSnapshot();

  if (typeof args1 === "function") {
    options["handler"] = args1;
  } else if (typeof args2 === "function") {
    options["handler"] = args2;
  } else if (
    typeof args1 === "object" && typeof args1["handler"] === "function"
  ) {
    options["handler"] = args1["handler"];
  } else {
    throw new TypeError("A handler function must be provided.");
  }

  if (typeof args1 === "object") {
    if (typeof args1["onListen"] === "function") {
      options["onListen"] = args1["onListen"];
    }
    if (typeof args1["onError"] === "function") {
      options["onError"] = args1["onError"];
    }
  }

  const handleHttp = async (conn) => {
    const currentHttpConn = serveHttp(conn);

    try {
      for await (const requestEvent of currentHttpConn) {
        ACTIVE_REQUESTS++;
        // NOTE: Respond to the request. Note we do not await this async
        // method to allow the connection to handle multiple requests in
        // the case of h2.
        //
        // [1]: https://deno.land/std@0.131.0/http/server.ts?source=#L338
        respond(requestEvent, currentHttpConn, options, snapshot).then(() => {
          ACTIVE_REQUESTS--;
        });
      }
    } catch {
      // connection has been closed
    } finally {
      closeHttpConn(currentHttpConn);
    }
  };

  const finished = (async () => {
    options["onListen"]?.({
      hostname: options.hostname,
      port: options.port,
    });

    for await (const conn of listener) {
      handleHttp(conn);
    }
  })();

  const kind = internals.worker.kind;
  const shutdownEventName = kind === "user" ? "drain" : "beforeunload";
  const handleShutdownEvent = () => {
    shutdown();
  };

  const shutdown = () => {
    removeEventListener(shutdownEventName, handleShutdownEvent);

    try {
      listener.close();
    } catch (error) {
      if (
        ObjectPrototypeIsPrototypeOf(BadResourcePrototype, error)
      ) {
        return;
      }

      throw error;
    }

    for (const httpConn of HTTP_CONNS) {
      closeHttpConn(httpConn);
    }
  };

  addEventListener(shutdownEventName, handleShutdownEvent, { once: true });

  return {
    finished,
    shutdown,
    ref() {
      // TODO: Not currently supported
    },
    unref() {
      // TODO: Not currently supported
    },
  };
}

async function respond(requestEvent, httpConn, options, snapshot) {
  const mapped = async function (requestEvent, httpConn, options, span) {
    /** @type {Response} */
    let response;

    const traceParent = requestEvent.request.headers.get("traceparent");
    let traceId = null;
    if (traceParent) {
      // traceparent format: 00-{trace-id}-{parent-id}-{flags}
      const parts = traceParent.split("-");
      if (parts.length >= 4 && parts[1].length === 32) {
        traceId = parts[1];
      }
    }
    const prevCtx = enterRequestContext(traceId);

    try {
      try {
        if (span) {
          updateSpanFromRequest(span, requestEvent.request);
        }

        response = await options["handler"](requestEvent.request, {
          remoteAddr: {
            port: options.port,
            hostname: options.hostname,
            transport: options.transport,
          },
        });
      } catch (error) {
        if (options["onError"] !== void 0) {
          /** @throwable */
          response = await options["onError"](error);
        } else {
          console.error(error);
          response = internalServerError();
        }
      }

      if (ObjectPrototypeIsPrototypeOf(ResponsePrototype, response) && span) {
        updateSpanFromResponse(span, response);
      }

      if (response === internals.RAW_UPGRADE_RESPONSE_SENTINEL) {
        const { fenceRid } = getSupabaseTag(requestEvent.request);

        if (fenceRid === void 0) {
          throw TypeError("Cannot find a fence for upgrading response");
        }

        setTimeout(async () => {
          const {
            status,
            headers,
          } = await ops.op_http_upgrade_raw2_fence(fenceRid);

          try {
            await requestEvent.respondWith(
              new Response(null, {
                headers,
                status,
              }),
            );
          } catch (error) {
            closeHttpConn(httpConn);
          }
        });
      } else {
        try {
          // send the response
          await requestEvent.respondWith(response);
        } catch {
          // respondWith() fails when the connection has already been closed,
          // or there is some other error with responding on this connection
          // that prompts us to close it and open a new connection.
          return closeHttpConn(httpConn);
        }
      }
    } finally {
      setAsyncContext(prevCtx);
    }
  };

  if (TRACING_ENABLED) {
    const oldSnapshot = currentSnapshot();
    restoreSnapshot(snapshot);

    const reqHeaders = requestEvent.request.headers;
    const headers = [];
    for (const key of reqHeaders.keys()) {
      ArrayPrototypePush(headers, [key, reqHeaders.get(key)]);
    }
    let activeContext = ContextManager.active();
    for (const propagator of new SafeArrayIterator(PROPAGATORS)) {
      activeContext = propagator.extract(activeContext, headers, {
        get(carrier, key) {
          return ArrayPrototypeFind(
            carrier,
            (carrierEntry) => carrierEntry[0] === key,
          )?.[1];
        },
        keys(carrier) {
          return ArrayPrototypeMap(
            carrier,
            (carrierEntry) => carrierEntry[0],
          );
        },
      });
    }

    const span = builtinTracer().startSpan(
      "deno.serve",
      { kind: 1 },
      activeContext,
    );
    enterSpan(span);
    try {
      return SafePromisePrototypeFinally(
        mapped(
          requestEvent,
          httpConn,
          options,
          span,
        ),
        () => span.end(),
      );
    } finally {
      restoreSnapshot(oldSnapshot);
    }
  } else {
    const oldSnapshot = currentSnapshot();
    restoreSnapshot(snapshot);
    try {
      return mapped(
        requestEvent,
        httpConn,
        options,
        undefined,
      );
    } finally {
      restoreSnapshot(oldSnapshot);
    }
  }
}

function closeHttpConn(httpConn) {
  try {
    httpConn.close();
  } catch {
    // connection has already been closed
  }
}

function getSupabaseTag(request) {
  return request[kSupabaseTag];
}

function applySupabaseTag(src, dest) {
  if (
    !ObjectPrototypeIsPrototypeOf(RequestPrototype, src) ||
    !ObjectPrototypeIsPrototypeOf(RequestPrototype, dest)
  ) {
    throw new TypeError("Only Request instance can apply the supabase tag");
  }

  dest[kSupabaseTag] = src[kSupabaseTag];
}

internals.getSupabaseTag = getSupabaseTag;
internals.RAW_UPGRADE_RESPONSE_SENTINEL = RAW_UPGRADE_RESPONSE_SENTINEL;

export { applySupabaseTag, getSupabaseTag, serve, serveHttp, upgradeWebSocket };
