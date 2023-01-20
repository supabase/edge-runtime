import { serve } from "https://deno.land/std@0.131.0/http/server.ts"

Deno.env.set("foo", "bar");

serve(async (req) => {
  const { name } = await req.json()
  const data = {
    message: `Hello ${name} from foo!`,
  }

  return new Response(
    JSON.stringify(data),
    { headers: { "Content-Type": "application/json", "Connection": "keep-alive" } },
  )
})
