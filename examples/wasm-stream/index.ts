import { serve } from 'https://deno.land/std@0.168.0/http/server.ts';
import { init } from 'https://esm.sh/@dqbd/tiktoken/lite/init';

await init(async (imports) => {
	return WebAssembly.instantiateStreaming(
		fetch('https://esm.sh/@dqbd/tiktoken@1.0.3/lite/tiktoken_bg.wasm'),
		imports,
	);
});

serve(async (req) => {
	return new Response(
		JSON.stringify({
			hello: 'world',
		}),
		{
			headers: { 'Content-Type': 'application/json' },
		},
	);
});
