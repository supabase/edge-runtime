import "ext:deno_http/01_http.js";

import { core, internals, primordials } from "ext:core/mod.js";
import { RequestPrototype } from "ext:deno_fetch/23_request.js";
import {
  fromInnerResponse,
  newInnerResponse,
} from "ext:deno_fetch/23_response.js";
import { upgradeWebSocket } from "ext:deno_http/02_websocket.ts";
import { HttpConn } from "ext:runtime/01_http.js";
import {
  builtinTracer,
  enterSpan,
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
  getAsyncContext,
  setAsyncContext,
} = core;
const { ObjectPrototypeIsPrototypeOf, SafePromisePrototypeFinally } =
  primordials;

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

    return nextRequest;
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
  const context = getAsyncContext();

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
        respond(requestEvent, currentHttpConn, options, context).then(() => {
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

async function respond(requestEvent, httpConn, options, context) {
  const mapped = async function (requestEvent, httpConn, options, span) {
    /** @type {Response} */
    let response;
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

    if (span) {
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
          console.error(error);
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
  };

  if (TRACING_ENABLED) {
    const oldContext = getAsyncContext();
    setAsyncContext(context);
    const span = builtinTracer().startSpan("deno.serve", { kind: 1 });
    try {
      enterSpan(span);
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
      // equiv to exitSpan.
      setAsyncContext(oldContext);
    }
  } else {
    const oldContext = getAsyncContext();
    setAsyncContext(context);
    try {
      return mapped(
        requestEvent,
        httpConn,
        options,
        undefined,
      );
    } finally {
      setAsyncContext(oldContext);
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
