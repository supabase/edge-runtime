// Regression fixture for supabase/edge-runtime#721.
// Deployed as artifact "B" for the SAME service path as marker_a.ts; the pool
// must not serve this request from a warm worker that is still running "A".
Deno.serve(() => new Response("MARKER_B"));
