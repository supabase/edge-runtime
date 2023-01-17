"use strict";

((window) => {
  const core = Deno.core;
  const ops = core.ops;
  const { ObjectDefineProperties }= window.__bootstrap.primordials;

  const base64 = window.__bootstrap.base64;
  const Console = window.__bootstrap.console.Console;
  const crypto = window.__bootstrap.crypto;
  const encoding = window.__bootstrap.encoding;
  const event = window.__bootstrap.event;
  const eventTarget = window.__bootstrap.eventTarget;
  const { env } = window.__bootstrap.os;
  const fetch = window.__bootstrap.fetch;
  const file = window.__bootstrap.file;
  const fileReader = window.__bootstrap.fileReader;
  const formData = window.__bootstrap.formData;
  const headers = window.__bootstrap.headers;
  const streams = window.__bootstrap.streams;
  const timers = window.__bootstrap.timers;
  const url = window.__bootstrap.url;
  const urlPattern = window.__bootstrap.urlPattern;
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
  }

  // Deno overrides
  Deno.env = env;
  Deno.listen = window.__bootstrap.net.listen;
  Deno.serveHttp = serveHttp;

  const __bootstrap = window.__bootstrap;
  delete window.__bootstrap;
  delete window.bootstrap;

  ObjectDefineProperties(window, globalScope);
})(this);

