// Exercises `UserWorker.fetch()` with request bodies that cannot be delivered:
// `/boot` + `/stale` dispatch to a worker that has already retired
// (supabase/edge-runtime#742) and `/early-response` forwards a cloned upload to
// a worker that answers without reading it (supabase/edge-runtime#737).
console.log("main function started");

type UserWorker = Awaited<ReturnType<typeof EdgeRuntime.userWorkers.create>>;

let worker: UserWorker | null = null;

function createWorker(name: string, workerTimeoutMs: number) {
  return EdgeRuntime.userWorkers.create({
    servicePath: `./test_cases/${name}`,
    memoryLimitMb: 150,
    workerTimeoutMs,
    cpuTimeSoftLimitMs: 10 * 60 * 1000,
    cpuTimeHardLimitMs: 10 * 60 * 1000,
    noModuleCache: false,
    envVars: [],
  });
}

function settle(
  promise: Promise<Response>,
  timeoutMs: number,
): Promise<Record<string, unknown>> {
  return Promise.race([
    promise.then(
      async (res) => {
        // Release the response body so the user worker sees the request end.
        await res.body?.cancel();
        return { kind: "response", status: res.status };
      },
      (err) => ({
        kind: "error",
        name: err?.name ?? typeof err,
        message: String(err),
      }),
    ),
    new Promise<Record<string, unknown>>((resolve) =>
      setTimeout(() => resolve({ kind: "timeout" }), timeoutMs)
    ),
  ]);
}

function intHeader(req: Request, key: string, fallback: number): number {
  const parsed = parseInt(req.headers.get(key) ?? "");
  return Number.isNaN(parsed) ? fallback : parsed;
}

Deno.serve(async (req: Request) => {
  const url = new URL(req.url);

  if (url.pathname === "/boot") {
    // The supervisor raises the retire flag at half of the wall clock limit
    // and terminates the worker when the limit has fully elapsed.
    worker = await createWorker(
      "sleep-5000ms",
      intHeader(req, "x-worker-timeout-ms", 4000),
    );

    // Keep an in-flight request on the worker so that it stays registered in
    // the pool after it retires instead of being shut down right away.
    const busy = new Request(url.origin + "/", { method: "GET" });
    EdgeRuntime.applySupabaseTag(req, busy);
    worker.fetch(busy).catch(() => {});

    return new Response("ok");
  }

  if (url.pathname === "/stale") {
    if (worker === null) {
      return new Response("boot first", { status: 400 });
    }

    const mode = req.headers.get("x-body-mode") ?? "none";
    const settleTimeoutMs = intHeader(req, "x-settle-timeout-ms", 2000);

    let target: Request;
    switch (mode) {
      case "none": {
        target = new Request(url.origin + "/", { method: "GET" });
        EdgeRuntime.applySupabaseTag(req, target);
        break;
      }
      case "passthrough": {
        target = req;
        break;
      }
      case "clone": {
        target = new Request(new URL(req.url), req.clone());
        EdgeRuntime.applySupabaseTag(req, target);
        break;
      }
      default:
        return new Response(`unknown mode: ${mode}`, { status: 400 });
    }

    const start = performance.now();
    const outcome = await settle(worker.fetch(target), settleTimeoutMs);

    return Response.json({
      ...outcome,
      elapsedMs: Math.round(performance.now() - start),
    });
  }

  // Forwards a cloned request to a worker that responds without reading the
  // request body, as a relay in front of a bounded upload endpoint would.
  if (url.pathname === "/early-response") {
    const settleTimeoutMs = intHeader(req, "x-settle-timeout-ms", 2000);
    const fresh = await createWorker("respond-without-reading-body", 60_000);
    const target = new Request(new URL(req.url), req.clone());
    EdgeRuntime.applySupabaseTag(req, target);

    const start = performance.now();
    const outcome = await settle(fresh.fetch(target), settleTimeoutMs);

    return Response.json({
      ...outcome,
      elapsedMs: Math.round(performance.now() - start),
    });
  }

  return new Response("not found", { status: 404 });
});
