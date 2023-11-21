// user worker state has a (tx, rx) for MessagePort
// main worker state has a (tx, rx) for MessagePort
//
// when main worker posts a message,

import { serve } from 'https://deno.land/std@0.131.0/http/server.ts';
interface reqPayload {
	name: string;
}

console.info('server started modified');

// const pendingJobsQueue = {};
//
// EdgeRuntime.messagePort.onmessage = (ev) => {
// 	//handle response
// 	const { id, result } = ev.data;
// 	pendingJobsQueue[id](result);
// };

serve(async (req: Request) => {
	const { name }: reqPayload = await req.json();
	const data = {
		message: `Hello ${name} from foo!`,
		test: 'foo',
	};

	return new Response(
		JSON.stringify(data),
		{ headers: { 'Content-Type': 'application/json', 'Connection': 'keep-alive' } },
	);
}, { port: 9005 });
