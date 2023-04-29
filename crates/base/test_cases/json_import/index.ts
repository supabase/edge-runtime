import file from "./version.json" assert { type: "json" };
import { serve } from "https://deno.land/std@0.131.0/http/server.ts"

serve(async (req: Request) => {
    return new Response(
        JSON.stringify({ version: file.version }),
        { status: 200, headers: { "Content-Type": "application/json" } },
    )
})
