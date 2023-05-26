import { serve } from "std/http/server.ts";
import foo from "foo/index.ts";

console.log(foo);

serve(async (req: Request) => {
  return new Response(
    JSON.stringify({ message: "ok" }),
    { headers: { "Content-Type": "application/json" } },
  );
});
