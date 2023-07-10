import { nextTick } from "node:process";
import { setImmediate } from "node:timers";
import { serve } from "https://deno.land/std@0.131.0/http/server.ts"

console.log("Hello => number 1");

setImmediate(() => {
    console.log("Running before the timeout => number 3");
});

setTimeout(() => {
    console.log("The timeout running last => number 4");
}, 0);

nextTick(() => {
    console.log("Running at next tick => number 2");
});

//
// Hello => number 1
//
// Trying to serve
//
// Running before the timeout => number 3
//
// The timeout running last => number 4
//
// Hello after request


serve(async (req: Request) => {

    setImmediate(() => {
        console.log('Hello after request');
    })

    return new Response(
        JSON.stringify({ hello: "world" }),
        { status: 200, headers: { "Content-Type": "application/json" } },
    )
});