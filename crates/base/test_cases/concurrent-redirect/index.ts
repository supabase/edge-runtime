// NOTE(Nyannyacha): This is the same test case as described in denoland/deno_core#762, but it is a
// minimal reproducible sample of what happens in the field.
//
// `@1.x` suffixes cause forced redirects for specifiers.

import * as A from "https://lib.deno.dev/x/grammy@1.x/mod.ts";
import * as B from "https://lib.deno.dev/x/grammy@1.x/types.ts";

console.log(A, B);

Deno.serve((_req) => new Response("meow"));
