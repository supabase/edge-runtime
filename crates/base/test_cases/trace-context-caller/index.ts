import { createClient } from "npm:@supabase/supabase-js@2.112.3";

const traceparent = "00-b1e669305668dc82c96cc31ce2e6cd99-1a5b74eca03f769f-01";

Deno.serve(async (req: Request) => {
  const origin = new URL(req.url).origin;
  const client = createClient(origin, "test-key", {
    auth: {
      persistSession: false,
      autoRefreshToken: false,
      detectSessionInUrl: false,
    },
  });

  const results = await Promise.all(
    ["fetch", "request", "invoke"].flatMap((method) =>
      ["present", "empty", "absent"].map(async (state) => {
        const headers: Record<string, string> = {
          // Exercise case-insensitive detection and an unrelated header.
          TraceParent: traceparent,
          "x-app-traceparent": traceparent,
          baggage: "caller=value",
        };
        if (state !== "absent") {
          headers.TraceState = state === "empty" ? "" : "caller=value";
        }

        let data;
        if (method === "invoke") {
          const result = await client.functions.invoke("callee", {
            headers,
            body: { hello: "world" },
          });
          if (result.error) throw result.error;
          data = result.data;
        } else {
          const url = `${origin}/callee`;
          const response = method === "request"
            ? await fetch(new Request(url, { headers }))
            : await fetch(url, { headers });
          if (!response.ok) throw new Error(await response.text());
          data = await response.json();
        }
        return { method, state, ...data };
      })
    ),
  );

  // Automatic propagation must still work when no context is supplied.
  const automatic = await fetch(`${origin}/callee`);
  if (!automatic.ok) throw new Error(await automatic.text());
  return Response.json({ results, automatic: await automatic.json() });
});
