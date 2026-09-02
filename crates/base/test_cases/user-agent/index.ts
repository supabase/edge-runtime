Deno.serve(async (req: Request) => {
  const echoUrl = req.headers.get("x-echo-url")!;
  const userAgent = req.headers.get("x-set-user-agent");

  // How the outbound `User-Agent` is handed to `fetch`. Each of these reaches
  // the header a different way, so each has to end up stamped.
  const style = req.headers.get("x-header-style");

  let resp: Response;

  if (userAgent === null) {
    resp = await fetch(echoUrl);
  } else if (style === "array") {
    resp = await fetch(echoUrl, { headers: [["user-agent", userAgent]] });
  } else if (style === "headers") {
    resp = await fetch(echoUrl, {
      headers: new Headers({ "user-agent": userAgent }),
    });
  } else if (style === "request") {
    resp = await fetch(
      new Request(echoUrl, { headers: { "user-agent": userAgent } }),
    );
  } else if (style === "request-overridden") {
    // `init.headers` wins over the ones the `Request` carries.
    resp = await fetch(
      new Request(echoUrl, { headers: { "user-agent": "discarded/1.0" } }),
      { headers: { "user-agent": userAgent } },
    );
  } else {
    resp = await fetch(echoUrl, { headers: { "user-agent": userAgent } });
  }

  return new Response(await resp.text());
});
