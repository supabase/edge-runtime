"use strict";

((window) => {
  const core = Deno.core;
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
  }= window.__bootstrap.primordials;

  const abortSignal = window.__bootstrap.abortSignal;
  const base64 = window.__bootstrap.base64;
  const Console = window.__bootstrap.console.Console;
  const colors = window.__bootstrap.colors;
  const crypto = window.__bootstrap.crypto;
  const domException = window.__bootstrap.domException;
  const encoding = window.__bootstrap.encoding;
  const event = window.__bootstrap.event;
  const { defineEventHandler } = window.__bootstrap.event;
  const eventTarget = window.__bootstrap.eventTarget;
  const { env } = window.__bootstrap.os;
  const fetch = window.__bootstrap.fetch;
  const file = window.__bootstrap.file;
  const fileReader = window.__bootstrap.fileReader;
  const formData = window.__bootstrap.formData;
  const globalInterfaces = window.__bootstrap.globalInterfaces;
  const headers = window.__bootstrap.headers;
  const inspectArgs = window.__bootstrap.console.inspectArgs;
  const streams = window.__bootstrap.streams;
  const timers = window.__bootstrap.timers;
  const url = window.__bootstrap.url;
  const urlPattern = window.__bootstrap.urlPattern;
  const webidl = window.__bootstrap.webidl;
  const webSocket = window.__bootstrap.webSocket;
  const net = window.__bootstrap.net_custom;
  const { HttpConn } = window.__bootstrap.http;

  function serveHttp(conn) {
    const rid = ops.op_http_start(conn.rid);
    return new HttpConn(rid, conn.remoteAddr, conn.localAddr);
  }

  // FIX: this should be done after snapshot
  Deno.core.initializeAsyncOps();

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
      new Console((msg, level) => core.print(msg, level > 1)),
    ),

    // timers
    clearInterval: writable(timers.clearInterval),
    clearTimeout: writable(timers.clearTimeout),
    setInterval: writable(timers.setInterval),
    setTimeout: writable(timers.setTimeout),

    // fetch
    Request: nonEnumerable(fetch.Request),
    Response: nonEnumerable(fetch.Response),
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
    EventTarget: nonEnumerable(eventTarget.EventTarget),
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
        return new domException.DOMException(msg, "OperationError");
      },
    );
    core.registerErrorBuilder(
      "DOMExceptionQuotaExceededError",
      function DOMExceptionQuotaExceededError(msg) {
        return new domException.DOMException(msg, "QuotaExceededError");
      },
    );
    core.registerErrorBuilder(
      "DOMExceptionNotSupportedError",
      function DOMExceptionNotSupportedError(msg) {
        return new domException.DOMException(msg, "NotSupported");
      },
    );
    core.registerErrorBuilder(
      "DOMExceptionNetworkError",
      function DOMExceptionNetworkError(msg) {
        return new domException.DOMException(msg, "NetworkError");
      },
    );
    core.registerErrorBuilder(
      "DOMExceptionAbortError",
      function DOMExceptionAbortError(msg) {
        return new domException.DOMException(msg, "AbortError");
      },
    );
    core.registerErrorBuilder(
      "DOMExceptionInvalidCharacterError",
      function DOMExceptionInvalidCharacterError(msg) {
        return new domException.DOMException(msg, "InvalidCharacterError");
      },
    );
    core.registerErrorBuilder(
      "DOMExceptionDataError",
      function DOMExceptionDataError(msg) {
        return new domException.DOMException(msg, "DataError");
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
  Deno.listen = window.__bootstrap.net.listen;
  Deno.connect = window.__bootstrap.net.connect;
  Deno.connectTls = window.__bootstrap.tls.connectTls;
  Deno.startTls = window.__bootstrap.tls.startTls;
  Deno.resolveDns = window.__bootstrap.net.resolveDns;
  Deno.serveHttp = serveHttp;

  const __bootstrap = window.__bootstrap;
  delete window.__bootstrap;
  delete window.bootstrap;

  ObjectDefineProperties(window, globalScope);

  const globalProperties = {
    Window: globalInterfaces.windowConstructorDescriptor,
    window: getterOnly(() => globalThis),
    self: getterOnly(() => globalThis),
  };
  ObjectDefineProperties(globalThis, globalProperties);
  ObjectSetPrototypeOf(globalThis, Window.prototype);

  // TODO: figure out if this is needed
  globalThis[webidl.brand] = webidl.brand;

  eventTarget.setEventTargetData(globalThis);

  defineEventHandler(window, "error");
  defineEventHandler(window, "load");
  defineEventHandler(window, "beforeunload");
  defineEventHandler(window, "unload");
  defineEventHandler(window, "unhandledrejection");

  core.setPromiseRejectCallback(promiseRejectCallback);

  runtimeStart({
    denoVersion: "NA",
    v8Version: "NA",
    tsVersion: "NA",
    noColor: true,
    isTty: false,
    target: window.__build_target,
  });
  delete window.__build_target;

  // set these overrides after runtimeStart
  ObjectDefineProperties(Deno, {
    build: readOnly(build),
    env: readOnly(env),
    pid: readOnly(window.__pid),
    args: readOnly([]), // args are set to be empty
    mainModule: getterOnly(opMainModule)
  });
  delete Deno.core;
})(this);

