const traceparent = "00-b1e669305668dc82c96cc31ce2e6cd99-1a5b74eca03f769f-01";

Deno.serve(async (req: Request) => {
  const origin = new URL(req.url).origin;

  const headers = {
    traceparent: traceparent,
    tracestate: "foo=bar",
    "x-app-traceparent": traceparent,
  };

  const response = await fetch(`${origin}/callee`, {
    headers,
    body: JSON.stringify({ hello: "world" }),
    method: "POST",
  });

  const data = await response.json();
  return Response.json({
    sent: traceparent,
    received: data,
  });
});