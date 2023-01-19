# Edge Runtime

This is a custom Edge Functions Runtime based off [Deno](https://deno.land), capable of running JavaScript, TypeScript, and WASM code.


### Why are we building this?

The primary goal of this project is to have a runtime environment that can simulate the capabilities and limitations of [Deno Deploy](https://deno.com/deploy).

This enables Supabase users to test their Edge Functions locally while simulating the behavior at the edge (eg: runtime APIs like File I/O not available, memory and CPU time enforced).
Also, this enables portability of edge functions to those users who want to self-host them outside of Deno Deploy.

## TODO

* TypeScript compile main module
* Cache modules (add a flag to disable it)
* Hot reload when a service changes
* Support import maps
* Check verify-jwt
* configuarble memory limit (update via flag)
* configurable wall clock limit (update via flag)
* better error messages for incorrect module loading paths (local)
* handle 404 errors
* Support snapshotting the runtime
