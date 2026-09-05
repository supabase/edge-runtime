// Regression fixture for supabase/edge-runtime#721.
// Deployed as artifact "A" for a given service path.
Deno.serve(() => new Response("MARKER_A"));
