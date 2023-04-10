# Supabase Edge Runtime

A web server based on [Deno](https://deno.land) runtime, capable of running JavaScript, TypeScript, and WASM services.

You can use it to:

* Locally test and self-host Supabase's Edge Functions (or any Deno Edge Function)
* As a programmable HTTP Proxy: You can intercept / route HTTP requests

**WARNING: Beta Software. There will be breaking changes to APIs / Configuration Options**

## How to run locally

```
./run.sh start --main-service /path/to/supabase/functions -p 9000
```

using Docker:

```
docker build -t edge-runtime .
docker run -it --rm -p 9000:9000 -v /path/to/supabase/functions:/usr/services supabase/edge-runtime start --main-service /usr/services
```

## How to run tests

make sure the docker daemon is running and create a docker image:

```bash
docker build -t edge-runtime:test .
```

install tests dependencies:

```bash
cd test
npm install
```

run tests:

```bash
npm run test
```

## How to update to a newer Deno version

* Select the Deno version to upgrade and visit its tag on GitHub (eg: https://github.com/denoland/deno/blob/v1.30.3/Cargo.toml)
* Open the `Cargo.toml` at the root of of this repo and modify all `deno_*` modules to match to the selected tag of Deno.

## Contributions

We welcome contributions to Supabase Edge Runtime!

To get started either open an issue on [GitHub](https://github.com/supabase/edge-runtime/issues) or drop us a message on [Discord](https://discord.com/invite/R7bSpeBSJE)

Edge Runtime follows Supabase's [Code of Conduct](https://github.com/supabase/.github/blob/main/CODE_OF_CONDUCT.md).
