# Usage & Self-Hosting

## The edge-runtime CLI

Edge runtime is based on [Deno](https://deno.land) and try to follows the same
concepts of it. One of them is the edge-runtime is primary a CLI based app and
have different commands as well parameters.

The easiest way to use edge-runtime is by running it from docker:

```bash
docker run --rm -it supabase/edge-runtime:v1.69.9
```

> The version above may be outdated.

<details>
  <summary>CLI output</summary>

```
A server based on Deno runtime, capable of running JavaScript, TypeScript, and WASM services

Usage: edge-runtime [OPTIONS] [COMMAND]

Commands:
start     Start the server
bundle    Creates an 'eszip' file that can be executed by the EdgeRuntime. Such file contains all the modules in contained in a single binary.
unbundle  Unbundles an .eszip file into the specified directory
help      Print this message or the help of the given subcommand(s)

Options:
-v, --verbose     Use verbose output
-q, --quiet       Do not print any log messages
    --log-source  Include source file and line in log messages
-h, --help        Print help
-V, --version     Print version
```

</details>

### `start` command

The start command allows you to run JavaScript/TypeScript code in a similar way of standard Deno.
But with the difference that it expects a main-service entrypoint with the given format: `path/index.ts`

<details>
  <summary>Example</summary>

```ts
// main.ts

console.log('Hello from Edge Runtime!!');
```

Running it from docker by using `--main-service` parameter:

```bash
docker run --rm -it -v $(pwd)/main.ts:/home/deno/main/index.ts supabase/edge-runtime:v1.69.9 start --main-service /home/deno/main
```

In the command above we did first map our local `main.ts` script to an `index.ts` file located at `/home/deno/main` in the docker volume.
So that we only need to supply this path as "main-service" entrypoint.

Edge runtime will then run the script and print the following output:

```
Hello from Edge Runtime!!
main worker has been destroyed
```

Notice that a *"main worker has been destroyed"* was printed out.
It means that our main service worker has nothing more to execute so the process will be finished.

</details>

## Edge functions

In the previous section we discussed in how `edge-runtime` cli can be used to run a JavaScript/TypeScript code.

**But how about serving edge functions?** In order to achieve that we must first understand the edge-runtime execution flow.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="/assets/edge-runtime-diagram-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="/assets/edge-runtime-diagram.svg">
    <img alt="Sequence diagram of Edge Runtime request flow" src="/assets/edge-runtime-diagram.svg" style="max-width: 100%;">
  </picture>
</p>

### Main Service

The main service is the initial script supplied on `start` command and it's should acts as root level of edge functions. By intercepting the incomming http request and spawing an **User Worker** to handle it.

All code execution here is more permisse, it means that main scope can access filesystem as well other privilegied APIs - like a `sudo` mode!

> When using Supabase Hosting Platform you don't have access to **Main Service** since its implicit managed by Supabase team.

### User Worker

Here's where your edge functions will really be executed!

The user worker is a more restricted and isolated environment, great to put user-specific code.

> When using `supabase functions deploy`, in a Supabase Hosting Platform, all your function code will be executed inside an **User Worker**.

### API Comparison

<details>
  <summary>Deno APIs</summary>

<https://docs.deno.com/api/deno>

| API  | Main Worker | User Worker |
|---|---|---|
| [`cloud`](https://docs.deno.com/api/deno/cloud) | ❌ | ❌ |
| [`fetch`](https://docs.deno.com/api/deno/fetch) | ✅ | ✅ |
| [`ffi`](https://docs.deno.com/api/deno/ffi) | ❌ | ❌ |
| [`file-system`](https://docs.deno.com/api/deno/file-system) | ✅ | ❌  |
| [`gpu`](https://docs.deno.com/api/deno/gpu) | ❌ | ❌ |
| [`http-server`](https://docs.deno.com/api/deno/http-server) | ✅ | ✅ |
| [`io`](https://docs.deno.com/api/deno/io) | ❌ | ❌ |
| [`jupyter`](https://docs.deno.com/api/deno/jupyter) | ❌ | ❌ |
| [`linter`](https://docs.deno.com/api/deno/linter) | ❌ | ❌ |
| [`network`](https://docs.deno.com/api/deno/network) | ✅ | ✅ |
| [`permissions`](https://docs.deno.com/api/deno/permissions) | ✅ | ✅ |
| [`runtime`](https://docs.deno.com/api/deno/runtime) | ⚠️ | ⚠️  |
| [`subprocess`](https://docs.deno.com/api/deno/subprocess) | ❌ | ❌ |
| [`telemetry`](https://docs.deno.com/api/deno/telemetry) | ✅ | ✅ |
| [`testing`](https://docs.deno.com/api/deno/testing) | ✅ | ✅ |
| [`websockets`](https://docs.deno.com/api/deno/websockets) | ✅ | ✅ |

> ❌ Not supported
> ✅ Supported
> ⚠️Partial supported

</details>

<details>
  <summary>Web APIs</summary>

  <https://docs.deno.com/api/web>

| API  | Main Worker | User Worker |
|---|---|---|
| [`cache`](https://docs.deno.com/api/web/cache) | ❌ | ⚠️ **ai models** only |
| [`canvas`](https://docs.deno.com/api/web/canvas) | ❌ | ❌ |
| [`crypto`](https://docs.deno.com/api/web/crypto) | ✅ | ✅ |
| [`encoding`](https://docs.deno.com/api/web/encoding) | ✅ | ✅ |
| [`events`](https://docs.deno.com/api/web/events) | ✅ | ✅ |
| [`events`](https://docs.deno.com/api/web/events) | ✅ | ✅ |
| [`fetch`](https://docs.deno.com/api/web/fetch) | ✅ | ✅ |
| [`file`](https://docs.deno.com/api/web/file) | ✅ | ✅ |
| [`gpu`](https://docs.deno.com/api/web/gpu) | ❌ | ❌ |
| [`io`](https://docs.deno.com/api/web/io) | ✅ | ✅ |
| [`io`](https://docs.deno.com/api/web/io) | ✅ | ✅ |
| [`intl`](https://docs.deno.com/api/web/intl) | ✅ | ✅ |
| [`messaging`](https://docs.deno.com/api/web/messaging) | ✅ | ✅ |
| [`performance`](https://docs.deno.com/api/web/performance) | ✅ | ✅ |
| [`platform`](https://docs.deno.com/api/web/platform) | ❌ | ❌ |
| [`storage`](https://docs.deno.com/api/web/storage) | ❌ | ❌ |
| [`streams`](https://docs.deno.com/api/web/streams) | ✅ | ✅ |
| [`temporal`](https://docs.deno.com/api/web/temporal) | ✅ | ✅ |
| [`url`](https://docs.deno.com/api/web/url) | ✅ | ✅ |
| [`wasm`](https://docs.deno.com/api/web/wasm) | ✅ | ✅ |
| [`websockets`](https://docs.deno.com/api/web/websockets) | ✅ | ✅ |
| [`workers`](https://docs.deno.com/api/web/storage) | ❌ | ❌ |

> ❌ Not supported
> ✅ Supported
> ⚠️Partial supported

</details>

<details>
  <summary>Node APIs</summary>

<https://docs.deno.com/api/node/>

| API  | Main Worker | User Worker |
|---|---|---|
| [`assert`](https://docs.deno.com/api/node/assert) | ✅ | ✅ |
| [`assert/strict`](https://docs.deno.com/api/node/assert/~/assert.strict) | ✅ | ✅ |
| [`assync_hooks`](https://docs.deno.com/api/node/assync_hooks) | ✅ | ✅ |
| [`buffer`](https://docs.deno.com/api/node/buffer) | ✅ | ✅ |
| [`child_process`](https://docs.deno.com/api/node/child_process) | ❌ | ❌ |
| [`cluster`](https://docs.deno.com/api/node/cluster) | ❌ | ❌ |
| [`console`](https://docs.deno.com/api/node/console) | ⚠️all outputs are trimmed out | ⚠️ same as *Main Worker* |
| [`crypto`](https://docs.deno.com/api/node/crypto) | ✅ | ✅ |
| [`dgram`](https://docs.deno.com/api/node/dgram) | ✅ | ✅ |
| [`diagnostics_channel`](https://docs.deno.com/api/node/diagnostics_channel) | ❌ | ❌ |
| [`dns`](https://docs.deno.com/api/node/dns) | ✅ | ✅ |
| [`dns/promisses`](https://docs.deno.com/api/node/dns/promisses) | ✅ | ✅ |
| [`domain`](https://docs.deno.com/api/node/domain) | ✅ | ✅ |
| [`events`](https://docs.deno.com/api/node/events) | ✅ | ✅ |
| [`fs`](https://docs.deno.com/api/node/fs) | ✅ | ❌ |
| [`fs/promisses`](https://docs.deno.com/api/node/fs/promisses) | ✅ | ❌ |
| [`http`](https://docs.deno.com/api/node/http) | ✅ | ✅ |
| [`http2`](https://docs.deno.com/api/node/http2) | ✅ | ✅ |
| [`https`](https://docs.deno.com/api/node/https) | ✅ | ✅ |
| [`inspector`](https://docs.deno.com/api/node/inspector) | ✅ | ✅ |
| [`inspector/promisses`](https://docs.deno.com/api/node/inspector/promisses) | ✅ | ✅ |
| [`module`](https://docs.deno.com/api/node/module) | ✅ | ✅ |
| [`net`](https://docs.deno.com/api/node/net) | ✅ | ✅ |
| [`os`](https://docs.deno.com/api/node/os) | ✅ | ✅ |
| [`path`](https://docs.deno.com/api/node/path) | ✅ | ✅ |
| [`perf_hooks`](https://docs.deno.com/api/node/perf_hooks) | ✅ | ✅ |
| [`process`](https://docs.deno.com/api/node/process) | ✅ | ✅ |
| [`querystring`](https://docs.deno.com/api/node/querystring) | ✅ | ✅ |
| [`readline`](https://docs.deno.com/api/node/readline) | ❌ | ❌ |
| [`readline/promisses`](https://docs.deno.com/api/node/readline/promisses) | ❌ | ❌ |
| [`repl`](https://docs.deno.com/api/node/repl) | ❌ | ❌ |
| [`sea`](https://docs.deno.com/api/node/sea) | ❌ | ❌ |
| [`sqlite`](https://docs.deno.com/api/node/sqlite) | ❌ | ❌ |
| [`stream`](https://docs.deno.com/api/node/stream) | ✅ | ✅ |
| [`stream/consumers`](https://docs.deno.com/api/node/stream/consumers) | ✅ | ✅ |
| [`stream/promisses`](https://docs.deno.com/api/node/stream/promisses) | ✅ | ✅ |
| [`stream/web`](https://docs.deno.com/api/node/stream/web) | ✅ | ✅ |
| [`string_decoder`](https://docs.deno.com/api/node/string_decoder) | ✅ | ✅ |
| [`test`](https://docs.deno.com/api/node/test) | ✅ | ✅ |
| [`test/reporters`](https://docs.deno.com/api/node/test/reporters) | ✅ | ✅ |
| [`timers`](https://docs.deno.com/api/node/timers) | ✅ | ✅ |
| [`timers/promisses`](https://docs.deno.com/api/node/timers/promisses) | ✅ | ✅ |
| [`tls`](https://docs.deno.com/api/node/tls) | ✅ | ✅ |
| [`trace_events`](https://docs.deno.com/api/node/trace_events) | ✅ | ✅ |
| [`tty`](https://docs.deno.com/api/node/tty) | ❌ | ❌ |
| [`url`](https://docs.deno.com/api/node/url) | ✅ | ✅ |
| [`util`](https://docs.deno.com/api/node/util) | ✅ | ✅ |
| [`util/types`](https://docs.deno.com/api/node/util/types) | ✅ | ✅ |
| [`v8`](https://docs.deno.com/api/node/v8) | ❌ | ❌ |
| [`vm`](https://docs.deno.com/api/node/vm) | ❌ | ❌ |
| [`wasi`](https://docs.deno.com/api/node/wasi) | ✅ | ✅ |
| [`worker_threads`](https://docs.deno.com/api/node/worker_threads) | ✅ | ✅ |
| [`zlib`](https://docs.deno.com/api/node/zlib) | ✅ | ✅ |

> ❌ Not supported
> ✅ Supported
> ⚠️Partial supported

</details>

### Self-Hosting

To self-host edge-functions you should manage the main service as well the user worker spawn logic, like [main.ts template](https://github.com/supabase/supabase/blob/d91ea9d4e24c211f666e6e0ff01d290a9f3831cb/docker/volumes/functions/main/index.ts)

The core idea is to have a root level `Deno.serve()` inside the main service and then foward the request to an user worker.

<details>
  <summary>Example</summary>

Creating a edge function to say hello!

```ts
// functions/hello-world/index.ts

Deno.serve(async (req: Request) => {
  const { name } = await req.json();

  const message = `Hello ${name} from foo!`;

  return Response.json({ message });
});
```

Handling http requests at main service level and passing it to a user worker:

```ts
// main/index.ts

import { exists } from 'jsr:@std/fs/exists';

Deno.serve(async (req: Request) => {
  console.log('new request', req.url); // http:localhost:9000/hello-world

  const edgeFunctionName = new URL(req.url).pathname; // "hello-world"

  // path relative to docker volume
  const edgeFunctionFilepath = `/home/deno/functions/${edgeFunctionName}`;

  // ensuring file exists
  if (!await exists(edgeFunctionFilepath)) {
    return new Response(null, { status: 404 });
  }

  try {
    // spawning a user worker
    const worker = await EdgeRuntime.userWorkers.create({
      servicePath: edgeFunctionFilepath,
      memoryLimitMb: 150,
      workerTimeoutMs: 1 * 60 * 1000,
    });

    // fowarding the request to user worker
    return await worker.fetch(req);
  } catch (error) {
    return Response.json({ error }, { status: 500 });
  }
});
```

Executing with docker

```bash
docker run --rm -it -p 9000:9000 -v $(pwd):/home/deno supabase/edge-runtime:v1.69.9 start --main-service /home/deno/main
```

Calling the edge function

```bash
$ curl localhost:9000/hello-world --data '{"name": "Kalleby Santos"}'

{"message":"Hello Kalleby Santos from foo!"}
```

</details>
