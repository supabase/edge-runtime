import { serve } from 'https://deno.land/std@0.168.0/http/server.ts';

serve(async (req) => {
	const { base } = await req.json();
	const data = mySlowFunction(base);
	return new Response(JSON.stringify(data), {
		headers: { 'Content-Type': 'application/json' },
	});
});

function mySlowFunction(baseNumber) {
	console.time('mySlowFunction');
	let now = Date.now();
	let result = 0;
	for (var i = Math.pow(baseNumber, 7); i >= 0; i--) {
		result += Math.atan(i) * Math.tan(i);
	}
	let duration = Date.now() - now;
	console.timeEnd('mySlowFunction');
	return { result: result, duration: duration };
}
