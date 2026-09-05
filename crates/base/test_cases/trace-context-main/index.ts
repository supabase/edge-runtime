Deno.serve(async (req: Request) => {
  const name = new URL(req.url).pathname.split("/").pop();
  if (name !== "caller" && name !== "callee") {
    return new Response("Not found", { status: 404 });
  }

  const worker = await EdgeRuntime.userWorkers.create({
    servicePath: `./test_cases/trace-context-${name}`,
    memoryLimitMb: 150,
    workerTimeoutMs: 60_000,
    cpuTimeSoftLimitMs: 60_000,
    cpuTimeHardLimitMs: 60_000,
    otelConfig: {
      tracing_enabled: true,
      propagators: ["TraceContext", "Baggage"],
    },
  });
  return await worker.fetch(req);
});