"use strict";

((window) => {
  const core = Deno.core;
  const ops = core.ops;
  const {
    ObjectDefineProperty,
    ObjectDefineProperties,
    ObjectPrototypeIsPrototypeOf,
    ObjectSetPrototypeOf,
    ObjectFreeze,
    StringPrototypeSplit
  }= window.__bootstrap.primordials;

  const abortSignal = window.__bootstrap.abortSignal;
  const base64 = window.__bootstrap.base64;
  const Console = window.__bootstrap.console.Console;
  const colors = window.__bootstrap.colors;
  const crypto = window.__bootstrap.crypto;
  const domException = window.__bootstrap.domException;
  const encoding = window.__bootstrap.encoding;
  //const errors = window.__bootstrap.errors.errors;
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
  const net = window.__bootstrap.net_custom;
  const { HttpConn } = window.__bootstrap.http;

  class HttpConnSingleRequest {
    #httpConn;
    #closed;

    constructor(rid, remoteAddr, localAddr) {
      this.#httpConn = new HttpConn(rid, remoteAddr, localAddr);
      this.#closed = false;
    }

    // Close the Http connection after serving.
    // We don't want to keep-alive the connection,
    // as next request may be intended to be served by another service.
    #close() {
      this.#httpConn.close();
      this.closed = true;
    }

    async nextRequest() {
      try {
        if (this.#closed) {
          return null;
        }

        let evt = await this.#httpConn.nextRequest();
        if (evt) {
          return {
            request: evt.request,
            respondWith: async (r) => {
              await evt.respondWith(r);
              this.#close();
            }
          }
        }
        return evt;
      } catch (e) {
        if (!e.message.includes("operation canceled")) {
          console.error(e);
        }
        return null;
      }
    }
  }

  function serveHttp(conn) {
    const rid = ops.op_http_start(conn.rid);
    return new HttpConnSingleRequest(rid, conn.remoteAddr, conn.localAddr);
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
  }

  //function registerErrors() {
  //  core.registerErrorClass("NotFound", errors.NotFound);
  //  core.registerErrorClass("PermissionDenied", errors.PermissionDenied);
  //  core.registerErrorClass("ConnectionRefused", errors.ConnectionRefused);
  //  core.registerErrorClass("ConnectionReset", errors.ConnectionReset);
  //  core.registerErrorClass("ConnectionAborted", errors.ConnectionAborted);
  //  core.registerErrorClass("NotConnected", errors.NotConnected);
  //  core.registerErrorClass("AddrInUse", errors.AddrInUse);
  //  core.registerErrorClass("AddrNotAvailable", errors.AddrNotAvailable);
  //  core.registerErrorClass("BrokenPipe", errors.BrokenPipe);
  //  core.registerErrorClass("AlreadyExists", errors.AlreadyExists);
  //  core.registerErrorClass("InvalidData", errors.InvalidData);
  //  core.registerErrorClass("TimedOut", errors.TimedOut);
  //  core.registerErrorClass("Interrupted", errors.Interrupted);
  //  core.registerErrorClass("WriteZero", errors.WriteZero);
  //  core.registerErrorClass("UnexpectedEof", errors.UnexpectedEof);
  //  core.registerErrorClass("BadResource", errors.BadResource);
  //  core.registerErrorClass("Http", errors.Http);
  //  core.registerErrorClass("Busy", errors.Busy);
  //  core.registerErrorClass("NotSupported", errors.NotSupported);
  //  core.registerErrorBuilder(
  //    "DOMExceptionOperationError",
  //    function DOMExceptionOperationError(msg) {
  //      return new domException.DOMException(msg, "OperationError");
  //    },
  //  );
  //  core.registerErrorBuilder(
  //    "DOMExceptionQuotaExceededError",
  //    function DOMExceptionQuotaExceededError(msg) {
  //      return new domException.DOMException(msg, "QuotaExceededError");
  //    },
  //  );
  //  core.registerErrorBuilder(
  //    "DOMExceptionNotSupportedError",
  //    function DOMExceptionNotSupportedError(msg) {
  //      return new domException.DOMException(msg, "NotSupported");
  //    },
  //  );
  //  core.registerErrorBuilder(
  //    "DOMExceptionNetworkError",
  //    function DOMExceptionNetworkError(msg) {
  //      return new domException.DOMException(msg, "NetworkError");
  //    },
  //  );
  //  core.registerErrorBuilder(
  //    "DOMExceptionAbortError",
  //    function DOMExceptionAbortError(msg) {
  //      return new domException.DOMException(msg, "AbortError");
  //    },
  //  );
  //  core.registerErrorBuilder(
  //    "DOMExceptionInvalidCharacterError",
  //    function DOMExceptionInvalidCharacterError(msg) {
  //      return new domException.DOMException(msg, "InvalidCharacterError");
  //    },
  //  );
  //  core.registerErrorBuilder(
  //    "DOMExceptionDataError",
  //    function DOMExceptionDataError(msg) {
  //      return new domException.DOMException(msg, "DataError");
  //    },
  //  );
  //}
  //

  function formatException(error) {
    if (ObjectPrototypeIsPrototypeOf(ErrorPrototype, error)) {
      return null;
    } else if (typeof error == "string") {
      return `Uncaught ${
        inspectArgs([quoteString(error)], {
          colors: !colors.getNoColor(),
        })
      }`;
    } else {
      return `Uncaught ${
        inspectArgs([error], { colors: !colors.getNoColor() })
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
    //core.setMacrotaskCallback(promiseRejectMacrotaskCallback);
    core.setWasmStreamingCallback(fetch.handleWasmStreaming);
    //core.setReportExceptionCallback(reportException);
    //ops.op_set_format_exception_callback(formatException);
    //version.setVersions(
    //  runtimeOptions.denoVersion,
    //  runtimeOptions.v8Version,
    //  runtimeOptions.tsVersion,
    //);
    setBuildInfo(runtimeOptions.target);
    //util.setLogDebug(runtimeOptions.debugFlag, source);
    colors.setNoColor(runtimeOptions.noColor || !runtimeOptions.isTty);

    // deno-lint-ignore prefer-primordials
    Error.prepareStackTrace = core.prepareStackTrace;

    // TODO: register errors
    //registerErrors();
  }

  // Deno overrides
  Deno.listen = window.__bootstrap.net.listen;
  Deno.serveHttp = serveHttp;

  const __bootstrap = window.__bootstrap;
  delete window.__bootstrap;
  delete window.bootstrap;

  ObjectDefineProperties(window, globalScope);

  ObjectDefineProperties(globalThis, {
    Window: globalInterfaces.windowConstructorDescriptor,
  });
  ObjectSetPrototypeOf(globalThis, Window.prototype);

  // TODO: figure out if this is needed
  globalThis[webidl.brand] = webidl.brand;

  eventTarget.setEventTargetData(globalThis);

  defineEventHandler(window, "error");
  defineEventHandler(window, "load");
  defineEventHandler(window, "beforeunload");
  defineEventHandler(window, "unload");
  defineEventHandler(window, "unhandledrejection");

  runtimeStart({
    denoVersion: "NA",
    v8Version: "NA",
    tsVersion: "NA",
    noColor: true,
    isTty: false,
    target: window.__build_target,
  });

  // set these overrides after runtimeStart
  ObjectDefineProperties(Deno, {
    env: readOnly(env),
    build: readOnly(build),
    pid: readOnly(window.__pid),
    ppid: readOnly(window.__ppid),
    args: readOnly([]), // args are set to be empty
    mainModule: getterOnly(opMainModule)
  });
})(this);

