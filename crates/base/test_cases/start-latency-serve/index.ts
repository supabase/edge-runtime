// Sets the header the runtime owns, to check that the runtime replaces it.
Deno.serve(() =>
  new Response("ok", { headers: { "sb-start-latency-us": "1" } })
);
