// Keep a failed handle to exercise dispatch after teardown as well as a fresh
// worker. HTTP keep-alive is controlled independently by the integration test.
let failedWorker;

Deno.serve(async (req: Request) => {
  const scenario = new URL(req.url).pathname.slice(1);
  try {
    const worker = scenario === 'retired' ? failedWorker : await EdgeRuntime.userWorkers.create({
      servicePath: scenario === 'cpu'
        ? '../../examples/cpu'
        : scenario === 'rejection'
        ? './test_cases/unhandled-rejection'
        : './test_cases/module-scope-throw',
      memoryLimitMb: 150,
      workerTimeoutMs: 10_000,
      cpuTimeSoftLimitMs: 100,
      cpuTimeHardLimitMs: 200,
      forceCreate: true,
      noModuleCache: false,
      envVars: [],
    });
    failedWorker = worker;
    return await worker.fetch(req);
  } catch (e) {
    return Response.json({ msg: e.toString() }, {
      status: 500,
      headers: { 'x-main-worker': 'recovered' },
    });
  }
});
