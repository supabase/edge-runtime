import { serve } from 'https://deno.land/std@0.131.0/http/server.ts';

interface reqPayload {
	name: string;
}

console.log('server started modified');

throw new Error('user function error');

serve(async (req: Request) => {
	const { name }: reqPayload = await req.json();
	const data = {
		message: `Hello ${name} from foo!`,
		test: 'foo',
	};

	const rand = Math.floor(Math.random() * 2);
	if (rand === 1) {
		throw new Error('user function error');
	}

	return new Response(
		JSON.stringify(data),
		{ headers: { 'Content-Type': 'application/json', 'Connection': 'keep-alive' } },
	);
}, { port: 9005 });
