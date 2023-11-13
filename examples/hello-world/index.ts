import { serve } from 'https://deno.land/std@0.131.0/http/server.ts';

interface reqPayload {
	name: string;
}

console.info('server started modified');

const pendingJobsQueue = {};

EdgeRuntime.messagePort.onmessage = (ev) => {
	//handle response
	const { id, result } = ev.data;
	pendingJobsQueue[id](result);
};

serve(async (req: Request) => {
	const { name }: reqPayload = await req.json();
	const data = {
		message: `Hello ${name} from foo!`,
		test: 'foo',
	};

	const promise = new Promise((resolve) => {
		EdgeRuntime.messagePort.postMessage({
			method: 'generateEmbeddings',
			input: 'hello world',
			id,
		});
		// add to queue
		pendingJobsQueue[id] = resolve;
	});

	// wait while the result is available
	const result = await promise;

	return new Response(
		JSON.stringify(data),
		{ headers: { 'Content-Type': 'application/json', 'Connection': 'keep-alive' } },
	);
}, { port: 9005 });
