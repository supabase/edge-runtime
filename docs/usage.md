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
