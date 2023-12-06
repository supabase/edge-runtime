import isEven from "npm:is-even";
import { serve } from "https://deno.land/std@0.131.0/http/server.ts"
import { hello } from "./hello.js";
import { numbers } from "./folder1/folder2/numbers.ts"

serve(async (req: Request) => {
    return new Response(
        JSON.stringify({ is_even: isEven(10), hello, numbers }),
        { status: 200, headers: { "Content-Type": "application/json" } },
    )
})