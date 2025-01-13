// @ts-ignore
import { STATUS_CODE } from 'https://deno.land/std/http/status.ts';

const SESSION_HEADER_NAME = 'X-Edge-Runtime-Session-Id';
const WORKERS = new Map<string, EdgeRuntime.UserWorker>();

setInterval(() => {
	const shouldBeRemoved: string[] = [];

	for (const [uuid, worker] of WORKERS) {
		if (!worker.active) {
			shouldBeRemoved.push(uuid);
		}
	}

	for (const uuid of shouldBeRemoved) {
		console.log("deleted: ", uuid);
		WORKERS.delete(uuid);
	}
}, 2500);

console.log('main function started (session mode)');

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

	if (pathname === '/_internal/metric') {
		const metric = await EdgeRuntime.getRuntimeMetrics();
		return Response.json(metric);
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
	const createWorker = async (): Promise<EdgeRuntime.UserWorker> => {
		const memoryLimitMb = 150;
		const workerTimeoutMs = 30 * 1000;
		const noModuleCache = false;

		const importMapPath = null;
		const envVarsObj = Deno.env.toObject();
		const envVars = Object.keys(envVarsObj).map((k) => [k, envVarsObj[k]]);
		const forceCreate = false;
		const netAccessDisabled = false;
		const cpuTimeSoftLimitMs = 10000;
		const cpuTimeHardLimitMs = 20000;

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
		});
	};

	const callWorker = async () => {

		try {
			let worker: EdgeRuntime.UserWorker | null = null;

			if (req.headers.get(SESSION_HEADER_NAME)) {
				const sessionId = req.headers.get(SESSION_HEADER_NAME)!;
				const complexSessionId = `${servicePath}/${sessionId}`;

				const maybeWorker = WORKERS.get(complexSessionId);

				if (maybeWorker && maybeWorker.active) {
					worker = maybeWorker;
				}
			}

			if (!worker) {
				worker = await createWorker();
			}

			const resp = await worker.fetch(req);

			if (resp.headers.has(SESSION_HEADER_NAME)) {
				const sessionIdFromWorker = resp.headers.get(SESSION_HEADER_NAME)!;
				const complexSessionId = `${servicePath}/${sessionIdFromWorker}`;

				WORKERS.set(complexSessionId, worker);
			}

			return resp;
		} catch (e) {
			console.error(e);

			if (e instanceof Deno.errors.WorkerRequestCancelled) {
				headers.append('Connection', 'close');
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
