import { serve } from "https://deno.land/std@0.131.0/http/server.ts"

serve(async (req: Request) => {
    return new Response(
        JSON.stringify({ hello: "world" }),
        { status: 400, headers: { "Content-Type": "application/json" } },
    )
})
