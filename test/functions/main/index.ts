import { serve } from 'https://deno.land/std@0.131.0/http/server.ts'

serve(async (req: Request) => {
  const url = new URL(req.url)
  const { pathname } = url
  const pathParts = pathname.split('/')
  const serviceName = pathParts[1]

  if (!serviceName || serviceName === '') {
    const error = { msg: 'missing function name in request' }
    return new Response(JSON.stringify(error), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    })
  }

  const servicePath = `/usr/services/${serviceName}`
  console.log(`serving the request with ${servicePath}`)

  const memoryLimitMb = 150
  const workerTimeoutMs = 5 * 60 * 1000
  const noModuleCache = false
  const importMapPath = `/usr/services/import_map.json`
  const envVars = []
  try {
    const worker = await EdgeRuntime.userWorkers.create({
      servicePath,
      memoryLimitMb,
      workerTimeoutMs,
      noModuleCache,
      importMapPath,
      envVars,
    })
    return worker.fetch(req)
  } catch (e) {
    const error = { msg: e.toString() }
    return new Response(JSON.stringify(error), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    })
  }
})
