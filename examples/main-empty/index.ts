import { serve } from "https://deno.land/std@0.131.0/http/server.ts";
import {sum} from "./some-import.ts";
import isEven from "npm:is-even";

console.log('Hello A');
globalThis.isTenEven = isEven(10);

console.log(Deno.version);
let val = sum(1, 2);
console.log(Deno.readFile)
console.log(Deno.cwd())
//console.log(Deno.readTextFileSync('/Users/andrespirela/Documents/workspace/supabase/edge-runtime/README.md'));

serve(async (req: Request) => {
    return new Response(
        JSON.stringify({ hello: "world" }),
        { status: 200, headers: { "Content-Type": "application/json" } },
    )
});