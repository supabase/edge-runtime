import { serve } from "https://deno.land/std@0.131.0/http/server.ts";
import {sum} from "./some-import.ts";

console.log(Deno.version);
let val = sum(1, 2);

serve(async (req: Request) => {
    return new Response(
        JSON.stringify({ hello: "world" }),
        { status: 200, headers: { "Content-Type": "application/json" } },
    )
});