// The path std's `serve` takes: Deno.listen and Deno.serveHttp, without Deno.serve.
for await (const conn of Deno.listen({ port: 9999 })) {
  (async () => {
    for await (const event of Deno.serveHttp(conn)) {
      await event.respondWith(new Response("ok"));
    }
  })();
}
