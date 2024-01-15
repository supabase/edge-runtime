Deno.serve(() => {
	// NOTE(Nyannyacha): This should be hot enough to V8 decides JIT compilation.

	const before = performance.now();
	const num = mySlowFunction(11);
	const after = performance.now();

	console.log(`time: ${after - before}ms`);

	return new Response(
		`meow: ${num} (${after - before})`,
		{
			status: 200
		}
	);
});

function mySlowFunction(baseNumber) {
	let result = 0;
	for (var i = Math.pow(baseNumber, 7); i >= 0; i--) {
		result += Math.atan(i) * Math.tan(i);
	};
	return result;
}
