console.log('main function started');

function parseIntFromHeadersOrDefault(req: Request, key: string, val: number) {
  const headerValue = req.headers.get(key);
  if (!headerValue) {
    return val;
  }

  const parsedValue = parseInt(headerValue);
  if (isNaN(parsedValue)) {
    return val;
  }

  return parsedValue;
}

Deno.serve((req: Request) => {
  console.log(req.url);
  const url = new URL(req.url);
  const { pathname } = url;
  const path_parts = pathname.split('/');
  const service_name = path_parts[1];

  if (!service_name || service_name === '') {
    const error = { msg: 'missing function name in request' };
    return new Response(
      JSON.stringify(error),
      { status: 400, headers: { 'Content-Type': 'application/json' } },
    );
  }

  const servicePath = `./test_cases/${service_name}`;
  console.error(`serving the request with ${servicePath}`);

  const createWorker = async () => {
    const memoryLimitMb = parseIntFromHeadersOrDefault(req, 'x-memory-limit-mb', 150);
    const workerTimeoutMs = parseIntFromHeadersOrDefault(
      req,
      'x-worker-timeout-ms',
      10 * 60 * 1000,
    );
    const cpuTimeSoftLimitMs = parseIntFromHeadersOrDefault(
      req,
      'x-cpu-time-soft-limit-ms',
      10 * 60 * 1000,
    );
    const cpuTimeHardLimitMs = parseIntFromHeadersOrDefault(
      req,
      'x-cpu-time-hard-limit-ms',
      10 * 60 * 1000,
    );
    const noModuleCache = false;
    const importMapPath = null;
    const envVarsObj = Deno.env.toObject();
    const envVars = Object.keys(envVarsObj).map((k) => [k, envVarsObj[k]]);
    const context = {
      sourceMap: req.headers.get('x-context-source-map') == 'true',
      useReadSyncFileAPI: req.headers.get('x-use-read-sync-file-api') == 'true',
    };

    return await EdgeRuntime.userWorkers.create({
      servicePath,
      memoryLimitMb,
      workerTimeoutMs,
      cpuTimeSoftLimitMs,
      cpuTimeHardLimitMs,
      noModuleCache,
      importMapPath,
      envVars,
      context,
    });
  };

  const callWorker = async () => {
    try {
      const worker = await createWorker();
      return await worker.fetch(req);
    } catch (e) {
      console.error(e);

      // if (e instanceof Deno.errors.WorkerRequestCancelled) {
      // 	return await callWorker();
      // }

      const error = { msg: e.toString() };
      return new Response(
        JSON.stringify(error),
        { status: 500, headers: { 'Content-Type': 'application/json' } },
      );
    }
  };

  return callWorker();
});
