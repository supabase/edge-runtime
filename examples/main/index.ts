import { serve } from "https://deno.land/std@0.131.0/http/server.ts"

console.log('main function started');

serve(async (req: Request) => {
  const url = new URL(req.url);
  const {pathname} = url;
  const path_parts = pathname.split("/");
  const service_name = path_parts[1];

  if (!service_name || service_name === "") {
    const error = { msg: "missing function name in request" }
    return new Response(
      JSON.stringify(error),
      { status: 400, headers: { "Content-Type": "application/json" } },
    )
  }

  const service_path = `./examples/${service_name}`;
  console.error(`serving the request with ${service_path}`);

  const memory_limit_mb = 150;
  const worker_timeout_ms = 5 * 60 * 1000;
  const no_module_cache = false;
  const import_map_path = null;
  const env_vars = [];
  try {
    const worker = await EdgeRuntime.userWorkers.create({
      service_path,
      memory_limit_mb,
      worker_timeout_ms,
      no_module_cache,
      import_map_path,
      env_vars
    });
    return worker.fetch(req);
  } catch (e) {
    const error = { msg: e.toString() }
    return new Response(
      JSON.stringify(error),
      { status: 500, headers: { "Content-Type": "application/json" } },
    )
  }
})
