# Supabase Edge Runtime

A web server based on [Deno](https://deno.land) runtime, capable of running JavaScript, TypeScript, and WASM services.
Edge Runtime is built and maintained by the [Supabase team](https://supabase.io). For more details, read the [intro blog post](https://supabase.com/blog/edge-runtime-self-hosted-deno-functions).

You can use it to:

* Locally test and self-host Supabase's Edge Functions (or any Deno Function)
* As a programmable HTTP Proxy: You can intercept / route HTTP requests

**WARNING: Beta Software. There will be breaking changes to APIs / Configuration Options**

## Architecture

![Sequence diagram of Edge Runtime request flow](https://github.com/supabase/edge-runtime/blob/main/edge-runtime-diagram.svg?raw=true)

The edge runtime can be divided into two runtimes with different purposes.
- Main runtime:
  - An instance for the _main runtime_ is responsible for proxying the transactions to the _user runtime_.
  - The main runtime is meant to be an entry point before running user functions, where you can authentication, etc. before calling the user function.
  - Has no user-facing limits
  - Has access to all environment variables.
- User runtime:
  - An instance for the _user runtime_ is responsible for executing users' code.
  - Limits are required to be set such as: Memory and Timeouts.
  - Has access to environment variables explictly allowed by the main runtime.

## How to run locally
To serve all functions in the examples folder on port 9000, you can do this with the [example main service](./examples/main/index.ts) provided with this repo
```sh
./scripts/run.sh start --main-service ./examples/main -p 9000
```

Test by calling the [hello world function](./examples/hello-world/index.ts)
```sh
curl --request POST 'http://localhost:9000/hello-world' \
--header 'Content-Type: application/json' \
--data-raw '{
    "name": "John Doe"
}'
```

To run with a different entry point, you can pass a different main service like below

```sh
./scripts/run.sh start --main-service /path/to/main-service-directory -p 9000
```

using Docker:

```
docker build -t edge-runtime .
docker run -it --rm -p 9000:9000 -v /path/to/supabase/functions:/functions supabase/edge-runtime start --main-service /functions/main
```

## How to run tests

```sh
./scripts/test.sh [TEST_NAME]
```

## How to update to a newer Deno version

* Select the Deno version to upgrade and visit its tag on GitHub (eg: https://github.com/denoland/deno/blob/v1.30.3/Cargo.toml)
* Open the `Cargo.toml` at the root of of this repo and modify all `deno_*` modules to match to the selected tag of Deno.

## Contributions

We welcome contributions to Supabase Edge Runtime!

To get started either open an issue on [GitHub](https://github.com/supabase/edge-runtime/issues) or drop us a message on [Discord](https://discord.com/invite/R7bSpeBSJE)

Edge Runtime follows Supabase's [Code of Conduct](https://github.com/supabase/.github/blob/main/CODE_OF_CONDUCT.md).
