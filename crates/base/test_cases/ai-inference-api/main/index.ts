Deno.serve(async (req: Request) => {
  const { pathname } = new URL(req.url);
  const servicePath = `./test_cases/ai-inference-api${pathname}`;

  try {
    const worker = await EdgeRuntime.userWorkers.create({
      servicePath,
      memoryLimitMb: 150,
      workerTimeoutMs: 60 * 1000,
      cpuTimeSoftLimitMs: 10 * 60 * 1000,
      cpuTimeHardLimitMs: 10 * 60 * 1000,
      noModuleCache: false,
      envVars: [
        ["AI_INFERENCE_API_HOST", req.headers.get("x-inference-host") ?? ""],
      ],
    });

    return await worker.fetch(req);
  } catch (e) {
    console.error(e);
    return Response.json({ msg: e.toString() }, { status: 500 });
  }
});
