import { serve } from "https://deno.land/std@0.131.0/http/server.ts";

console.log("main function started");

serve(async (req: Request) => {
  const url = new URL(req.url);
  const { pathname } = url;

  // handle health checks
  if (pathname === "/_internal/health") {
    return new Response(
      JSON.stringify({ "message": "ok" }),
      { status: 200, headers: { "Content-Type": "application/json" } },
    );
  }

  const path_parts = pathname.split("/");
  const service_name = path_parts[1];

  if (!service_name || service_name === "") {
    const error = { msg: "missing function name in request" };
    return new Response(
      JSON.stringify(error),
      { status: 400, headers: { "Content-Type": "application/json" } },
    );
  }

  const servicePath = `./examples/${service_name}`;
  console.error(`serving the request with ${servicePath}`);

  const createWorker = async () => {
    const memoryLimitMb = 150;
    const workerTimeoutMs = 5 * 60 * 1000;
    const noModuleCache = false;
    const customModuleRoot = null;
    // you can provide an import map inline
    // const inlineImportMap = {
    //   imports: {
    //     "std/": "https://deno.land/std@0.131.0/",
    //     "cors": "./examples/_shared/cors.ts"
    //   }
    // }
    // const importMapPath = `data:${encodeURIComponent(JSON.stringify(importMap))}?${encodeURIComponent('/home/deno/functions/test')}`;
    const importMapPath = null;
    const envVarsObj = Deno.env.toObject();
    envVarsObj['DENO_CERT'] = `-----BEGIN CERTIFICATE-----
MIIG4zCCBcugAwIBAgIJAMZV+9j3/9xlMA0GCSqGSIb3DQEBCwUAMIG0MQswCQYD
VQQGEwJVUzEQMA4GA1UECBMHQXJpem9uYTETMBEGA1UEBxMKU2NvdHRzZGFsZTEa
MBgGA1UEChMRR29EYWRkeS5jb20sIEluYy4xLTArBgNVBAsTJGh0dHA6Ly9jZXJ0
cy5nb2RhZGR5LmNvbS9yZXBvc2l0b3J5LzEzMDEGA1UEAxMqR28gRGFkZHkgU2Vj
dXJlIENlcnRpZmljYXRlIEF1dGhvcml0eSAtIEcyMB4XDTIzMDQxNDAyMzkzMVoX
DTI0MDQxMjAyNTg1NVowZjELMAkGA1UEBhMCSEsxDzANBgNVBAcTBkpvcmRhbjEv
MC0GA1UEChMmWXVlIEh3YSBDaGluZXNlIFByb2R1Y3RzIEVtcG9yaXVtIEx0ZC4x
FTATBgNVBAMMDCoueXVlaHdhLmNvbTCCASIwDQYJKoZIhvcNAQEBBQADggEPADCC
AQoCggEBALj0pg9thV9dVkL+vCCyHITqP0U9R8zhuTabG9QE4znUrWMXPRT3cIZ5
UN4tCRz5HEMZ/9Jkfi7ixfgg0e+Gnw0hRknyMZ7O9dfBLe89Q3dE9OM9n2bsES5Y
1pIpLqbME0lMDSnt8SIY5hhowTE8bOtHCVhkalVlA5pVHYbna3v9mD1qcL2fRBWa
ChTOxFda0mKciLm2RxDiQIMVk9pJytEznIyDTb4tlMVWj3BhhYqE6+MiZ3Ucl7OS
C9wJ8XW+eAm3gew8IiLMmyY2JlJVF2kvz/qo1hmo93dWX5WyLbIxiLnj83smxzKF
iTtqrhEsBUDGcUpa4GGLLnvuwIqPa0sCAwEAAaOCA0MwggM/MAwGA1UdEwEB/wQC
MAAwKQYDVR0lBCIwIAYIKwYBBQUHAwEGCCsGAQUFBwMCBgpghkgBhvhNAQIDMA4G
A1UdDwEB/wQEAwIFoDA2BgNVHR8ELzAtMCugKaAnhiVodHRwOi8vY3JsLmdvZGFk
ZHkuY29tL2dkaWcyczItMzMuY3JsMF0GA1UdIARWMFQwSAYLYIZIAYb9bQEHFwIw
OTA3BggrBgEFBQcCARYraHR0cDovL2NlcnRpZmljYXRlcy5nb2RhZGR5LmNvbS9y
ZXBvc2l0b3J5LzAIBgZngQwBAgIwdgYIKwYBBQUHAQEEajBoMCQGCCsGAQUFBzAB
hhhodHRwOi8vb2NzcC5nb2RhZGR5LmNvbS8wQAYIKwYBBQUHMAKGNGh0dHA6Ly9j
ZXJ0aWZpY2F0ZXMuZ29kYWRkeS5jb20vcmVwb3NpdG9yeS9nZGlnMi5jcnQwHwYD
VR0jBBgwFoAUQMK9J47MNIMwojPX+2yz8LQsgM4wIwYDVR0RBBwwGoIMKi55dWVo
d2EuY29tggp5dWVod2EuY29tMB0GA1UdDgQWBBRsyuW07PjUJcvcRsgn7n+E0X20
LzCCAX4GCisGAQQB1nkCBAIEggFuBIIBagFoAHYA7s3QZNXbGs7FXLedtM0TojKH
Rny87N7DUUhZRnEftZsAAAGHfaHbmwAABAMARzBFAiEAiTcOqM3PVqc3EejXctCA
xUhTxmXirfsVdmkiSQ5uLZcCICh8aeq67gConeEveV01LLa4MUfm3CA+k5njqs2Z
w/F2AHYASLDja9qmRzQP5WoC+p0w6xxSActW3SyB2bu/qznYhHMAAAGHfaHcZAAA
BAMARzBFAiBWnxRSeA89+ipoG5aKS0QuZyD63wfVfClEkyM+mh17TAIhAN6K2Hvj
ezsLRAW2DgiUdLb5z+/qirlCrDng+GjWRBuaAHYA2ra/az+1tiKfm8K7XGvocJFx
bLtRhIU0vaQ9MEjX+6sAAAGHfaHcwgAABAMARzBFAiAFHuUMTQCHJTDGgEFlgxSb
VbP3M+strCsK+aLAAPC8PwIhAIOwj1UTkzReRVZD2wznGBOYp0xEA/CGzkDa+IGo
dIoVMA0GCSqGSIb3DQEBCwUAA4IBAQBplNlP80kVaV2/OnVK+iiefqUHmCShd2tY
eenoFo8ZbsJJ0OHSstudsivruC7Wv9h6vTJk2lmTQVzvlAOUMpSoF4zF2t/k85W1
aHtrE66yXu1nvdXT5uslEnkAuw00N9FUD3vh1v8YYui1fgqYPL4yz7uDP2Anmpld
7eKrJrR9YFCNxeWEZ+IqsGNN59QEEQNXs3dY4ztL1G8O63syp+yaLb9Mtb01PCcI
3X4wRa1WVlSYajTfCKLzaofuLBGDSa+PQeCq+bYQ455PrPBImVeOriEwgjPvDO0l
7fSmf5eoRdvGi8IcAdS8zdY3QUjyJNbduGKqW1qq0kCbEmA0BBvC
-----END CERTIFICATE-----`;

    const envVars = Object.keys(envVarsObj).map((k) => [k, envVarsObj[k]]);
    const forceCreate = false;

    return await EdgeRuntime.userWorkers.create({
      servicePath,
      memoryLimitMb,
      workerTimeoutMs,
      noModuleCache,
      importMapPath,
      envVars,
      forceCreate,
      customModuleRoot,
    });
  };

  const callWorker = async () => {
    try {
      // If a worker for the given service path already exists,
      // it will be reused by default.
      // Update forceCreate option in createWorker to force create a new worker for each request.
      const worker = await createWorker();
      return await worker.fetch(req);
    } catch (e) {
      console.error(e);
      const error = { msg: e.toString() };
      return new Response(
        JSON.stringify(error),
        { status: 500, headers: { "Content-Type": "application/json" } },
      );
    }
  };

  return callWorker();
});
