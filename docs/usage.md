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
