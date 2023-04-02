import { serve } from "https://deno.land/std@0.131.0/http/server.ts"

serve(async (req) => {
  let status = 200;
  if (req.method == "PUT") {
    status = 405;
  }

  console.log("received request", req.url);
  console.log("headers", req.headers);

  // const { name } = await req.json()
  // const data = {
  //   message: `Hello ${name} from bar!`,
  // }

  return new Response(
    JSON.stringify({"helo": "world"}),
    { headers: { "Content-Type": "application/json", "x-custom": "bar" }, status },
  )
})
