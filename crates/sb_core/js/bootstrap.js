import * as abortSignal from "ext:deno_web/03_abort_signal.js";
import * as base64 from "ext:deno_web/05_base64.js";
import * as console from "ext:deno_console/02_console.js";
import * as crypto from "ext:deno_crypto/00_crypto.js";
import DOMException from "ext:deno_web/01_dom_exception.js";
import * as encoding from "ext:deno_web/08_text_encoding.js";
import * as event from "ext:deno_web/02_event.js";
import * as fetch from "ext:deno_fetch/26_fetch.js";
import * as file from "ext:deno_web/09_file.js";
import * as fileReader from "ext:deno_web/10_filereader.js";
import * as formData from "ext:deno_fetch/21_formdata.js";
import * as colors from "ext:deno_console/01_colors.js";
import * as headers from "ext:deno_fetch/20_headers.js";
import * as streams from "ext:deno_web/06_streams.js";
import * as timers from "ext:deno_web/02_timers.js";
import * as url from "ext:deno_url/00_url.js";
import * as urlPattern from "ext:deno_url/01_urlpattern.js";
import * as webidl from "ext:deno_webidl/00_webidl.js";
import * as webSocket from "ext:deno_websocket/01_websocket.js";
import { HttpConn } from "ext:deno_http/01_http.js";
import * as tls from "ext:deno_net/02_tls.js";
import * as net from "ext:deno_net/01_net.js";
import * as response from "ext:deno_fetch/23_response.js";
import * as request from "ext:deno_fetch/23_request.js";
import { SUPABASE_USER_WORKERS } from "ext:sb_user_workers/user_workers.js";
import { SUPABASE_ENV } from "ext:sb_env/env.js";


const core = globalThis.Deno.core;
const ops = core.ops;

const {
  ArrayPrototypeIndexOf,
  ArrayPrototypePush,
  ArrayPrototypeShift,
  ArrayPrototypeSplice,
  Error,
  ErrorPrototype,
  ObjectDefineProperty,
  ObjectDefineProperties,
  ObjectPrototypeIsPrototypeOf,
  ObjectSetPrototypeOf,
  ObjectFreeze,
  SafeWeakMap,
  StringPrototypeSplit,
  WeakMapPrototypeGet,
  WeakMapPrototypeSet,
  WeakMapPrototypeDelete
}= globalThis.__bootstrap.primordials;

const defineEventHandler = event.defineEventHandler;

function serveHttp(conn) {
  const rid = ops.op_http_start(conn.rid);
  return new HttpConn(rid, conn.remoteAddr, conn.localAddr);
}

function nonEnumerable(value) {
  return {
    value,
    writable: true,
    enumerable: false,
    configurable: true,
  };
}

function writable(value) {
  return {
    value,
    writable: true,
    enumerable: true,
    configurable: true,
  };
}

function readOnly(value) {
  return {
    value,
    enumerable: true,
    writable: false,
    configurable: true,
  };
}

function getterOnly(getter) {
  return {
    get: getter,
    set() {},
    enumerable: true,
    configurable: true,
  };
}

const globalScope = {
  console: nonEnumerable(
      new console.Console((msg, level) => core.print(msg, level > 1)),
  ),

  // timers
  clearInterval: writable(timers.clearInterval),
  clearTimeout: writable(timers.clearTimeout),
  setInterval: writable(timers.setInterval),
  setTimeout: writable(timers.setTimeout),

  // fetch
  Request: nonEnumerable(request.Request),
  Response: nonEnumerable(response.Response),
  Headers: nonEnumerable(headers.Headers),
  fetch: writable(fetch.fetch),

  // base64
  atob: writable(base64.atob),
  btoa: writable(base64.btoa),

  // encoding
  TextDecoder: nonEnumerable(encoding.TextDecoder),
  TextEncoder: nonEnumerable(encoding.TextEncoder),
  TextDecoderStream: nonEnumerable(encoding.TextDecoderStream),
  TextEncoderStream: nonEnumerable(encoding.TextEncoderStream),

  // url
  URL: nonEnumerable(url.URL),
  URLPattern: nonEnumerable(urlPattern.URLPattern),
  URLSearchParams: nonEnumerable(url.URLSearchParams),

  // crypto
  CryptoKey: nonEnumerable(crypto.CryptoKey),
  crypto: readOnly(crypto.crypto),
  Crypto: nonEnumerable(crypto.Crypto),
  SubtleCrypto: nonEnumerable(crypto.SubtleCrypto),

  // streams
  ByteLengthQueuingStrategy: nonEnumerable(
      streams.ByteLengthQueuingStrategy,
  ),
  CountQueuingStrategy: nonEnumerable(
      streams.CountQueuingStrategy,
  ),
  ReadableStream: nonEnumerable(streams.ReadableStream),
  ReadableStreamDefaultReader: nonEnumerable(
      streams.ReadableStreamDefaultReader,
  ),
  ReadableByteStreamController: nonEnumerable(
      streams.ReadableByteStreamController,
  ),
  ReadableStreamBYOBReader: nonEnumerable(
      streams.ReadableStreamBYOBReader,
  ),
  ReadableStreamBYOBRequest: nonEnumerable(
      streams.ReadableStreamBYOBRequest,
  ),
  ReadableStreamDefaultController: nonEnumerable(
      streams.ReadableStreamDefaultController,
  ),
  TransformStream: nonEnumerable(streams.TransformStream),
  TransformStreamDefaultController: nonEnumerable(
      streams.TransformStreamDefaultController,
  ),
  WritableStream: nonEnumerable(streams.WritableStream),
  WritableStreamDefaultWriter: nonEnumerable(
      streams.WritableStreamDefaultWriter,
  ),
  WritableStreamDefaultController: nonEnumerable(
      streams.WritableStreamDefaultController,
  ),

  // event
  CloseEvent: nonEnumerable(event.CloseEvent),
  CustomEvent: nonEnumerable(event.CustomEvent),
  ErrorEvent: nonEnumerable(event.ErrorEvent),
  Event: nonEnumerable(event.Event),
  EventTarget: nonEnumerable(event.EventTarget),
  MessageEvent: nonEnumerable(event.MessageEvent),
  PromiseRejectionEvent: nonEnumerable(event.PromiseRejectionEvent),
  ProgressEvent: nonEnumerable(event.ProgressEvent),
  reportError: writable(event.reportError),

  // file
  Blob: nonEnumerable(file.Blob),
  File: nonEnumerable(file.File),
  FileReader: nonEnumerable(fileReader.FileReader),

  // form data
  FormData: nonEnumerable(formData.FormData),

  // abort signal
  AbortController: nonEnumerable(abortSignal.AbortController),
  AbortSignal: nonEnumerable(abortSignal.AbortSignal),

  // web sockets
  WebSocket: nonEnumerable(webSocket.WebSocket),
}

const pendingRejections = [];
const pendingRejectionsReasons = new SafeWeakMap();

function promiseRejectCallback(type, promise, reason) {
  switch (type) {
    case 0: {
      ops.op_store_pending_promise_rejection(promise, reason);
      ArrayPrototypePush(pendingRejections, promise);
      WeakMapPrototypeSet(pendingRejectionsReasons, promise, reason);
      break;
    }
    case 1: {
      ops.op_remove_pending_promise_rejection(promise);
      const index = ArrayPrototypeIndexOf(pendingRejections, promise);
      if (index > -1) {
        ArrayPrototypeSplice(pendingRejections, index, 1);
        WeakMapPrototypeDelete(pendingRejectionsReasons, promise);
      }
      break;
    }
    default:
      return false;
  }

  return !!globalThis.onunhandledrejection ||
      event.listenerCount(globalThis, "unhandledrejection") > 0;
}


function promiseRejectMacrotaskCallback() {
  while (pendingRejections.length > 0) {
    const promise = ArrayPrototypeShift(pendingRejections);
    const hasPendingException = ops.op_has_pending_promise_rejection(
        promise,
    );
    const reason = WeakMapPrototypeGet(pendingRejectionsReasons, promise);
    WeakMapPrototypeDelete(pendingRejectionsReasons, promise);

    if (!hasPendingException) {
      continue;
    }

    const rejectionEvent = new event.PromiseRejectionEvent(
        "unhandledrejection",
        {
          cancelable: true,
          promise,
          reason,
        },
    );

    const errorEventCb = (event) => {
      if (event.error === reason) {
        ops.op_remove_pending_promise_rejection(promise);
      }
    };
    // Add a callback for "error" event - it will be dispatched
    // if error is thrown during dispatch of "unhandledrejection"
    // event.
    globalThis.addEventListener("error", errorEventCb);
    globalThis.dispatchEvent(rejectionEvent);
    globalThis.removeEventListener("error", errorEventCb);

    // If event was not prevented (or "unhandledrejection" listeners didn't
    // throw) we will let Rust side handle it.
    if (rejectionEvent.defaultPrevented) {
      ops.op_remove_pending_promise_rejection(promise);
    }
  }
  return true;
}

class NotFound extends Error {
  constructor(msg) {
    super(msg);
    this.name = "NotFound";
  }
}

class PermissionDenied extends Error {
  constructor(msg) {
    super(msg);
    this.name = "PermissionDenied";
  }
}

class ConnectionRefused extends Error {
  constructor(msg) {
    super(msg);
    this.name = "ConnectionRefused";
  }
}

class ConnectionReset extends Error {
  constructor(msg) {
    super(msg);
    this.name = "ConnectionReset";
  }
}

class ConnectionAborted extends Error {
  constructor(msg) {
    super(msg);
    this.name = "ConnectionAborted";
  }
}

class NotConnected extends Error {
  constructor(msg) {
    super(msg);
    this.name = "NotConnected";
  }
}

class AddrInUse extends Error {
  constructor(msg) {
    super(msg);
    this.name = "AddrInUse";
  }
}

class AddrNotAvailable extends Error {
  constructor(msg) {
    super(msg);
    this.name = "AddrNotAvailable";
  }
}

class BrokenPipe extends Error {
  constructor(msg) {
    super(msg);
    this.name = "BrokenPipe";
  }
}

class AlreadyExists extends Error {
  constructor(msg) {
    super(msg);
    this.name = "AlreadyExists";
  }
}

class InvalidData extends Error {
  constructor(msg) {
    super(msg);
    this.name = "InvalidData";
  }
}

class TimedOut extends Error {
  constructor(msg) {
    super(msg);
    this.name = "TimedOut";
  }
}

class WriteZero extends Error {
  constructor(msg) {
    super(msg);
    this.name = "WriteZero";
  }
}

class WouldBlock extends Error {
  constructor(msg) {
    super(msg);
    this.name = "WouldBlock";
  }
}

class UnexpectedEof extends Error {
  constructor(msg) {
    super(msg);
    this.name = "UnexpectedEof";
  }
}

class Http extends Error {
  constructor(msg) {
    super(msg);
    this.name = "Http";
  }
}

class Busy extends Error {
  constructor(msg) {
    super(msg);
    this.name = "Busy";
  }
}

class NotSupported extends Error {
  constructor(msg) {
    super(msg);
    this.name = "NotSupported";
  }
}

function registerErrors() {
  core.registerErrorClass("NotFound", NotFound);
  core.registerErrorClass("PermissionDenied", PermissionDenied);
  core.registerErrorClass("ConnectionRefused", ConnectionRefused);
  core.registerErrorClass("ConnectionReset", ConnectionReset);
  core.registerErrorClass("ConnectionAborted", ConnectionAborted);
  core.registerErrorClass("NotConnected", NotConnected);
  core.registerErrorClass("AddrInUse", AddrInUse);
  core.registerErrorClass("AddrNotAvailable", AddrNotAvailable);
  core.registerErrorClass("BrokenPipe", BrokenPipe);
  core.registerErrorClass("AlreadyExists", AlreadyExists);
  core.registerErrorClass("InvalidData", InvalidData);
  core.registerErrorClass("TimedOut", TimedOut);
  core.registerErrorClass("Interrupted", core.Interrupted);
  core.registerErrorClass("WriteZero", WriteZero);
  core.registerErrorClass("UnexpectedEof", UnexpectedEof);
  core.registerErrorClass("BadResource", core.BadResource);
  core.registerErrorClass("Http", Http);
  core.registerErrorClass("Busy", Busy);
  core.registerErrorClass("NotSupported", NotSupported);
  core.registerErrorBuilder(
      "DOMExceptionOperationError",
      function DOMExceptionOperationError(msg) {
        return new DOMException(msg, "OperationError");
      },
  );
  core.registerErrorBuilder(
      "DOMExceptionQuotaExceededError",
      function DOMExceptionQuotaExceededError(msg) {
        return new DOMException(msg, "QuotaExceededError");
      },
  );
  core.registerErrorBuilder(
      "DOMExceptionNotSupportedError",
      function DOMExceptionNotSupportedError(msg) {
        return new DOMException(msg, "NotSupported");
      },
  );
  core.registerErrorBuilder(
      "DOMExceptionNetworkError",
      function DOMExceptionNetworkError(msg) {
        return new DOMException(msg, "NetworkError");
      },
  );
  core.registerErrorBuilder(
      "DOMExceptionAbortError",
      function DOMExceptionAbortError(msg) {
        return new DOMException(msg, "AbortError");
      },
  );
  core.registerErrorBuilder(
      "DOMExceptionInvalidCharacterError",
      function DOMExceptionInvalidCharacterError(msg) {
        return new DOMException(msg, "InvalidCharacterError");
      },
  );
  core.registerErrorBuilder(
      "DOMExceptionDataError",
      function DOMExceptionDataError(msg) {
        return new DOMException(msg, "DataError");
      },
  );
}


function formatException(error) {
  if (ObjectPrototypeIsPrototypeOf(ErrorPrototype, error)) {
    return null;
  } else if (typeof error == "string") {
    return `Uncaught ${
        inspectArgs([quoteString(error)], {
          colors: false,
        })
    }`;
  } else {
    return `Uncaught ${
        inspectArgs([error], { colors: false })
    }`;
  }
}

// set build info
const build = {
  target: "unknown",
  arch: "unknown",
  os: "unknown",
  vendor: "unknown",
  env: undefined,
};

function setBuildInfo(target) {
  const { 0: arch, 1: vendor, 2: os, 3: env } = StringPrototypeSplit(
      target,
      "-",
      4,
  );
  build.target = target;
  build.arch = arch;
  build.vendor = vendor;
  build.os = os;
  build.env = env;
  ObjectFreeze(build);
}

function opMainModule() {
  return ops.op_main_module();
}

function runtimeStart(runtimeOptions, source) {
  core.setMacrotaskCallback(timers.handleTimerMacrotask);
  core.setMacrotaskCallback(promiseRejectMacrotaskCallback);
  core.setWasmStreamingCallback(fetch.handleWasmStreaming);
  //core.setReportExceptionCallback(reportException);
  ops.op_set_format_exception_callback(formatException);
  //version.setVersions(
  //  runtimeOptions.denoVersion,
  //  runtimeOptions.v8Version,
  //  runtimeOptions.tsVersion,
  //);
  setBuildInfo(runtimeOptions.target);
  colors.setNoColor(runtimeOptions.noColor || !runtimeOptions.isTty);

  // deno-lint-ignore prefer-primordials
  Error.prepareStackTrace = core.prepareStackTrace;

  registerErrors();
}

// Deno overrides
Deno.listen = net.listen;
Deno.connect = net.connect;
Deno.connectTls = tls.connectTls;
Deno.startTls = tls.startTls;
Deno.resolveDns = net.resolveDns;
Deno.serveHttp = serveHttp;

// EdgeRuntime namespace
// FIXME: Make the object read-only
globalThis.EdgeRuntime = {
  userWorkers: SUPABASE_USER_WORKERS
};

const __bootstrap = globalThis.__bootstrap;
delete globalThis.__bootstrap;
delete globalThis.bootstrap;

ObjectDefineProperties(globalThis, globalScope);

// TODO: figure out if this is needed
globalThis[webidl.brand] = webidl.brand;

event.setEventTargetData(globalThis);

defineEventHandler(globalThis, "error");
defineEventHandler(globalThis, "load");
defineEventHandler(globalThis, "beforeunload");
defineEventHandler(globalThis, "unload");
defineEventHandler(globalThis, "unhandledrejection");

core.setPromiseRejectCallback(promiseRejectCallback);

runtimeStart({
  denoVersion: "NA",
  v8Version: "NA",
  tsVersion: "NA",
  noColor: true,
  isTty: false,
  target: "supabase",
});
delete globalThis.__build_target;

// set these overrides after runtimeStart
ObjectDefineProperties(Deno, {
  build: readOnly(build),
  env: readOnly(SUPABASE_ENV),
  pid: readOnly(globalThis.__pid),
  args: readOnly([]), // args are set to be empty
  mainModule: getterOnly(opMainModule)
});

// TODO: Abstract this file into multiple files. There's too much boilerplate
