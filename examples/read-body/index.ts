import { serve } from 'https://deno.land/std@0.131.0/http/server.ts';

interface reqPayload {
	name: string;
}

console.info('server started modified');

serve(async (req: Request) => {
	console.log('serving request');
	const rb = req.body;
	const reader = rb.getReader();

	let totalSize = 0;
	const readReq = async () => {
		const { done, value } = await reader.read();
		if (!done) {
			totalSize += value.length;
			return await readReq();
		}
		return;
	};
	await readReq();

	return new Response(
		JSON.stringify({ totalSize }),
		{ headers: { 'Content-Type': 'application/json', 'Connection': 'keep-alive' } },
	);
}, { port: 9005 });
