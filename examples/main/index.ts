// @ts-ignore
import { STATUS_CODE } from 'https://deno.land/std/http/status.ts';

import { handleRegistryRequest } from './registry/mod.ts';

console.log('main function started');

addEventListener('beforeunload', () => {
    console.log('main worker exiting');
});

// log system memory usage every 30s
// setInterval(() => console.log(EdgeRuntime.systemMemoryInfo()), 30 * 1000);

// cleanup unused sessions every 30s
// setInterval(async () => {
//     try {
//         const cleanupCount = await EdgeRuntime.ai.tryCleanupUnusedSession();
//         if (cleanupCount == 0) {
//             return;
//         }
//         console.log('EdgeRuntime.ai.tryCleanupUnusedSession', cleanupCount);
//     } catch (e) {
//         console.error(e.toString());
//     }
// }, 30 * 1000);

Deno.serve(async (req: Request) => {
    const headers = new Headers({
        'Content-Type': 'application/json',
    });

    const url = new URL(req.url);
    const { pathname } = url;

    // handle health checks
    if (pathname === '/_internal/health') {
        return new Response(
            JSON.stringify({ 'message': 'ok' }),
            {
                status: STATUS_CODE.OK,
                headers,
            },
        );
    }

    // if (pathname === '/_internal/segfault') {
    // 	EdgeRuntime.raiseSegfault();
    // 	return new Response(
    // 		JSON.stringify({ 'message': 'ok' }),
    // 		{
    // 			status: STATUS_CODE.OK,
    // 			headers,
    // 		},
    // 	);
    // }

    if (pathname === '/_internal/metric') {
        const metric = await EdgeRuntime.getRuntimeMetrics();
        return Response.json(metric);
    }

    // NOTE: You can test WebSocket in the main worker by uncommenting below.
    // if (pathname === '/_internal/ws') {
    // 	const upgrade = req.headers.get("upgrade") || "";

    // 	if (upgrade.toLowerCase() != "websocket") {
    // 		return new Response("request isn't trying to upgrade to websocket.");
    // 	}

    // 	const { socket, response } = Deno.upgradeWebSocket(req);

    // 	socket.onopen = () => console.log("socket opened");
    // 	socket.onmessage = (e) => {
    // 		console.log("socket message:", e.data);
    // 		socket.send(new Date().toString());
    // 	};

    // 	socket.onerror = e => console.log("socket errored:", e.message);
    // 	socket.onclose = () => console.log("socket closed");

    // 	return response; // 101 (Switching Protocols)
    // }

    const REGISTRY_PREFIX = '/_internal/registry';
    if (pathname.startsWith(REGISTRY_PREFIX)) {
        return await handleRegistryRequest(REGISTRY_PREFIX, req);
    }

    const path_parts = pathname.split('/');
    const service_name = path_parts[1];

    if (!service_name || service_name === '') {
        const error = { msg: 'missing function name in request' };
        return new Response(
            JSON.stringify(error),
            { status: STATUS_CODE.BadRequest, headers: { 'Content-Type': 'application/json' } },
        );
    }

    const servicePath = `./examples/${service_name}`;
    // console.error(`serving the request with ${servicePath}`);

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
        //const maybeEszip = await Deno.readFile('./bin.eszip');
        //const maybeEntrypoint = 'file:///src/index.ts';

        // const maybeEntrypoint = 'file:///src/index.ts';
        // or load module source from an inline module
        // const maybeModuleCode = 'Deno.serve((req) => new Response("Hello from Module Code"));';
        //
        const cpuTimeSoftLimitMs = 10000;
        const cpuTimeHardLimitMs = 20000;
        const staticPatterns = [
            './examples/**/*.html',
        ];

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
            staticPatterns,
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

            if (e instanceof Deno.errors.WorkerRequestCancelled) {
                headers.append('Connection', 'close');

                // XXX(Nyannyacha): I can't think right now how to re-poll
                // inside the worker pool without exposing the error to the
                // surface.

                // It is satisfied when the supervisor that handled the original
                // request terminated due to reaches such as CPU time limit or
                // Wall-clock limit.
                //
                // The current request to the worker has been canceled due to
                // some internal reasons. We should repoll the worker and call
                // `fetch` again.

                // return await callWorker();
            }

            const error = { msg: e.toString() };
            return new Response(
                JSON.stringify(error),
                {
                    status: STATUS_CODE.InternalServerError,
                    headers,
                },
            );
        }
    };

    return callWorker();
});
