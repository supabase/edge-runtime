import { core, internals } from "ext:core/mod.js";

const { AsyncVariable } = core;

const requestTraceIdVar = new AsyncVariable();

// Header injected by the runtime's fetch / node:http into every outbound
// request to propagate the request-local trace ID for rate limiting.
// Defined once here and shared via internals to avoid scattered string literals.
const FETCH_TRACE_ID_HEADER = "x-er-fetch-trace-id";

function enterRequestContext(traceId) {
  return requestTraceIdVar.enter(traceId);
}

function getRequestTraceId() {
  return requestTraceIdVar.get();
}

internals.enterRequestContext = enterRequestContext;
internals.getRequestTraceId = getRequestTraceId;
internals.FETCH_TRACE_ID_HEADER = FETCH_TRACE_ID_HEADER;

export { enterRequestContext, FETCH_TRACE_ID_HEADER, getRequestTraceId };
