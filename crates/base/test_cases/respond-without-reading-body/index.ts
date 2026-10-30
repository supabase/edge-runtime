// Answers immediately and never touches the request body, like an endpoint
// that rejects an upload up front based on its headers.
Deno.serve(() => {
  return new Response("payload too large", { status: 413 });
});
