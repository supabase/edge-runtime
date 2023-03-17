# Edge Runtime

A web server based on [Deno](https://deno.land) runtime, capable of running JavaScript, TypeScript, and WASM services.

### Why are we building this?

The primary goal of this project is to have a runtime environment that can simulate the capabilities and limitations of [Deno Deploy](https://deno.com/deploy).

This enables Supabase users to test their Edge Functions locally while simulating the behavior at the edge (eg: runtime APIs like File I/O not available, memory and CPU time enforced).
Also, this enables portability of edge functions to those users who want to self-host them outside of Deno Deploy.

## How to run locally

```
./run.sh start --main-service /path/to/supabase/functions -p 9000
```

using Docker:

```
docker build -t edge-runtime .
docker run -it --rm -p 9000:9000 -v /path/to/supabase/functions:/usr/services supabase/edge-runtime start --main-service /usr/services
```

## TODO

* Expose Deno.errors
* Performance.now() precision tuning
* Disable SharedArrayBuffers
* better error messages for incorrect module loading paths
* better error messages for invalid import map paths
* Support snapshotting the runtime
* Support for private modules (DENO_AUTH_TOKENS)
* HTTP/2 support (need the host to support SSL)
* Add tests
* Add a benchmarking suite

## How to update to a Deno version

* Select the Deno version to upgrade and visit its tag on GitHub (eg: https://github.com/denoland/deno/blob/v1.30.3/Cargo.toml)
* Open the `Cargo.toml` at the root of of this repo and modify all `deno_*` modules to match to the selected tag of Deno.

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
