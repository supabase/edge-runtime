import "@supabase/functions-js/edge-runtime.d.ts";

console.log("meow");
Deno.serve((_req) => new Response("Hello, world"));
