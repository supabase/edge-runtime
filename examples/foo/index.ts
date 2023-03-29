import { serve } from "https://deno.land/std@0.131.0/http/server.ts"

interface reqPayload {
  name: string;
}

console.log('server started modified');

serve(async (req: Request) => {
  const memory_limit_mb = 150;
  const worker_timeout_ms = 60 * 1000 * 30;
  const no_module_cache = false;
  const import_map_path = null;
  const env_vars = [];
  const rest = await EdgeRuntime.userWorkers.create('./examples/hono', memory_limit_mb, worker_timeout_ms, no_module_cache, import_map_path, env_vars);
  console.log('worker key', rest);
  //return EdgeRuntime.userWorkers.serveRequest("./examples/bar", req);

  const { name } : reqPayload = await req.json()
  const data = {
    message: `Hello ${name} from foo!`,
    test: 'foo'
  }

  return new Response(
    JSON.stringify(data),
    { headers: { "Content-Type": "application/json", "Connection": "keep-alive" } },
  )
}, { port: 9005 })
