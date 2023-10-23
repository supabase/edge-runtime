import isEven from "npm:is-even";
import { serve } from "https://deno.land/std@0.131.0/http/server.ts"

serve(async (req: Request) => {
    return new Response(
        JSON.stringify({ is_even: isEven(10) }),
        { status: 200, headers: { "Content-Type": "application/json" } },
    )
})