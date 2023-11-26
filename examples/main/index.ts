import { serve } from 'https://deno.land/std@0.131.0/http/server.ts';

console.log('main function started');

serve(async (req: Request) => {
	const url = new URL(req.url);
	const { pathname } = url;

	// handle health checks
	if (pathname === '/_internal/health') {
		return new Response(
			JSON.stringify({ 'message': 'ok' }),
			{ status: 200, headers: { 'Content-Type': 'application/json' } },
		);
	}

	const path_parts = pathname.split('/');
	const service_name = path_parts[1];

	if (!service_name || service_name === '') {
		const error = { msg: 'missing function name in request' };
		return new Response(
			JSON.stringify(error),
			{ status: 400, headers: { 'Content-Type': 'application/json' } },
		);
	}

	const servicePath = `./examples/${service_name}`;
	console.error(`serving the request with ${servicePath}`);

	const createWorker = async () => {
		const memoryLimitMb = 150;
		const workerTimeoutMs = 5 * 60 * 1000;
		const noModuleCache = false;
		// you can provide an import map inline
		// const inlineImportMap = {
		//   imports: {
		//     "std/": "https://deno.land/std@0.131.0/",
		//     "cors": "./examples/_shared/cors.ts"
		//   }
		// }
		// const importMapPath = `data:${encodeURIComponent(JSON.stringify(importMap))}?${encodeURIComponent('/home/deno/functions/test')}`;
		const importMapPath = null;
		const envVarsObj = Deno.env.toObject();
		const envVars = Object.keys(envVarsObj).map((k) => [k, envVarsObj[k]]);
		const forceCreate = false;
		const netAccessDisabled = false;

		// load source from an eszip
		// const maybeEszip = await Deno.readFile('./sample.eszip');
		// const maybeEntrypoint = 'file:///src/index.ts';
		// or load module source from an inline module
		// const maybeModuleCode = 'Deno.serve((req) => new Response("Hello from Module Code"));';
		//
		const cpuTimeSoftLimitMs = 50;
		const cpuTimeHardLimitMs = 100;

		return await EdgeRuntime.userWorkers.create({
			servicePath,
			memoryLimitMb,
			workerTimeoutMs,
			noModuleCache,
			importMapPath,
			envVars,
			forceCreate,
			netAccessDisabled,
			cpuTimeSoftLimitMs,
			cpuTimeHardLimitMs,
			// maybeEszip,
			// maybeEntrypoint,
			// maybeModuleCode,
		});
	};

	const callWorker = async () => {
		try {
			// If a worker for the given service path already exists,
			// it will be reused by default.
			// Update forceCreate option in createWorker to force create a new worker for each request.
			const worker = await createWorker();
			const controller = new AbortController();

			const signal = controller.signal;
			// Optional: abort the request after a timeout
			//setTimeout(() => controller.abort(), 2 * 60 * 1000);

			return await worker.fetch(req, { signal });
		} catch (e) {
			console.error(e);
			const error = { msg: e.toString() };
			return new Response(
				JSON.stringify(error),
				{ status: 500, headers: { 'Content-Type': 'application/json' } },
			);
		}
	};

	return callWorker();
});
