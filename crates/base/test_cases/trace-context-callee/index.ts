Deno.serve((req: Request) =>
  Response.json({
    traceparent: req.headers.get("traceparent"),
    tracestate: req.headers.get("tracestate"),
    baggage: req.headers.get("baggage"),
    custom: req.headers.get("x-app-traceparent"),
  })
);