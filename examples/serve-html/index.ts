import { join } from 'https://deno.land/std/path/mod.ts';

Deno.serve(async (req) => {
	if (req.url.endsWith('/foo')) {
		return new Response(await Deno.readTextFile(join(import.meta.dirname, 'foo.html')));
	} else {
		return new Response(await Deno.readTextFile(join(import.meta.dirname, 'bar.html')));
	}
});
