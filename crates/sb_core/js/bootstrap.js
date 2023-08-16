import * as abortSignal from "ext:deno_web/03_abort_signal.js";
import * as base64 from "ext:deno_web/05_base64.js";
import * as console from "ext:deno_console/01_console.js";
import * as crypto from "ext:deno_crypto/00_crypto.js";
import DOMException from "ext:deno_web/01_dom_exception.js";
import * as encoding from "ext:deno_web/08_text_encoding.js";
import * as event from "ext:deno_web/02_event.js";
import * as fetch from "ext:deno_fetch/26_fetch.js";
import * as file from "ext:deno_web/09_file.js";
import * as fileReader from "ext:deno_web/10_filereader.js";
import * as formData from "ext:deno_fetch/21_formdata.js";
import * as headers from "ext:deno_fetch/20_headers.js";
import * as streams from "ext:deno_web/06_streams.js";
import * as timers from "ext:deno_web/02_timers.js";
import * as url from "ext:deno_url/00_url.js";
import * as urlPattern from "ext:deno_url/01_urlpattern.js";
import * as webidl from "ext:deno_webidl/00_webidl.js";
import * as webSocket from "ext:deno_websocket/01_websocket.js";
import * as response from "ext:deno_fetch/23_response.js";
import * as request from "ext:deno_fetch/23_request.js";
import * as globalInterfaces from "ext:deno_web/04_global_interfaces.js";
import { SUPABASE_ENV } from "ext:sb_env/env.js";
import {
  registerErrors
} from "ext:sb_core_main_js/js/errors.js";
import {
  nonEnumerable,
  writable,
  readOnly,
  getterOnly,
  formatException
} from "ext:sb_core_main_js/js/fieldUtils.js";
import { Navigator, navigator, setNumCpus, setLanguage, setUserAgent } from "ext:sb_core_main_js/js/navigator.js"
import { promiseRejectMacrotaskCallback } from "ext:sb_core_main_js/js/promises.js";
import { fsVars, denoOverrides } from "ext:sb_core_main_js/js/denoOverrides.js";
import * as performance from "ext:deno_web/15_performance.js";
import * as messagePort from "ext:deno_web/13_message_port.js";
import { SupabaseEventListener } from "ext:sb_user_event_worker/event_worker.js";
import * as MainWorker from "ext:sb_core_main_js/js/main_worker.js";
import * as DenoWebCompression from "ext:deno_web/14_compression.js";
import * as DenoWSStream from "ext:deno_websocket/02_websocketstream.js";

const core = globalThis.Deno.core;
const ops = core.ops;

const {
  Error,
  ObjectDefineProperty,
  ObjectDefineProperties,
  ObjectSetPrototypeOf,
  ObjectFreeze,
  StringPrototypeSplit,
} = globalThis.__bootstrap.primordials;

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
  DOMException: nonEnumerable(DOMException),

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

  // performance
  Performance: nonEnumerable(performance.Performance),
  PerformanceEntry: nonEnumerable(performance.PerformanceEntry),
  PerformanceMark: nonEnumerable(performance.PerformanceMark),
  PerformanceMeasure: nonEnumerable(performance.PerformanceMeasure),
  performance: writable(performance.performance),

  // messagePort
  structuredClone: writable(messagePort.structuredClone),

  // Branding as a WebIDL object
  [webidl.brand]: nonEnumerable(webidl.brand),
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

function runtimeStart(runtimeOptions, source) {
  core.setMacrotaskCallback(timers.handleTimerMacrotask);
  core.setMacrotaskCallback(promiseRejectMacrotaskCallback);
  core.setWasmStreamingCallback(fetch.handleWasmStreaming);

  ops.op_set_format_exception_callback(formatException);

  setBuildInfo(runtimeOptions.target);

  // deno-lint-ignore prefer-primordials
  Error.prepareStackTrace = core.prepareStackTrace;

  registerErrors();
}

// We need to delete globalThis.console
// Before setting up a new one
// This is because v8 sets a console that can't be easily overriden
// and collides with globalScope.console
delete globalThis.console;
ObjectDefineProperties(globalThis, globalScope);

const globalProperties = {
    Window: globalInterfaces.windowConstructorDescriptor,
    window: getterOnly(() => globalThis),
    Navigator: nonEnumerable(Navigator),
    navigator: getterOnly(() => navigator),
    self: getterOnly(() => globalThis),
};

ObjectDefineProperties(globalThis, globalProperties);
ObjectSetPrototypeOf(globalThis, Window.prototype);
event.setEventTargetData(globalThis);

const eventHandlers = ["error", "load", "beforeunload", "unload", "unhandledrejection"];
eventHandlers.forEach((handlerName) => event.defineEventHandler(globalThis, handlerName));

const deleteDenoApis = (apis) => {
    apis.forEach((key) => {
       delete Deno[key];
    });
}

globalThis.bootstrapSBEdge = (opts, isUserWorker, isEventsWorker) => {

  // We should delete this after initialization,
  // Deleting it during bootstrapping can backfire
  delete globalThis.__bootstrap;
  delete globalThis.bootstrap;

  runtimeStart({
    denoVersion: "NA",
    v8Version: "NA",
    tsVersion: "NA",
    noColor: true,
    isTty: false,
    ...opts
  });

  // set these overrides after runtimeStart
  ObjectDefineProperties(denoOverrides, {
    build: readOnly(build),
    env: readOnly(SUPABASE_ENV),
    pid: readOnly(globalThis.__pid),
    args: readOnly([]), // args are set to be empty
    mainModule: getterOnly(() => ops.op_main_module()),
  });
  ObjectDefineProperty(globalThis, "Deno", readOnly(denoOverrides));

  setNumCpus(1); // explicitly setting no of CPUs to 1 (since we don't allow workers)
  setUserAgent("Supabase Edge Runtime");
  setLanguage("en");


  if(isUserWorker) {
    delete globalThis.EdgeRuntime;

    // override console
    ObjectDefineProperties(globalThis, { console: nonEnumerable(
      new console.Console((msg, level) => {
        return ops.op_user_worker_log(msg, level > 1, ops.op_runtime_memory_usage())
      }),
    )});

    deleteDenoApis(Object.keys(fsVars));
  }

  if(isEventsWorker) {
    // Event Manager should have the same as the `main` except it can't create workers (that would be catastrophic)
    delete globalThis.EdgeRuntime;
    ObjectDefineProperties(globalThis, {
      EventManager: getterOnly(() => SupabaseEventListener)
    })
  }

  delete globalThis.bootstrapSBEdge;
}
