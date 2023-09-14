import { serve } from "https://deno.land/std@0.131.0/http/server.ts";

console.log(Deno.version);

serve(async (req: Request) => {
    return new Response(
        JSON.stringify({ hello: "world" }),
        { status: 200, headers: { "Content-Type": "application/json" } },
    )
})
