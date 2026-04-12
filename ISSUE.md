# Native Node addons (.node) not supported in Supabase Edge Runtime

## Summary

`@duckdb/node-api` (which depends on `@duckdb/node-bindings`) works correctly under standalone Deno 2.1.4 but fails on `supabase-edge-runtime-1.73.3` (also Deno 2.1.4 compatible) because the edge runtime does not support loading native Node addons (`.node` files).

## Environment

- **supabase CLI**: 2.89.1 (via npm)
- **edge-runtime**: supabase-edge-runtime-1.73.3 (compatible with Deno v2.1.4)
- **local Deno**: 2.1.4
- **@duckdb/node-api**: 1.5.1-r.1
- **@duckdb/node-bindings**: 1.5.1-r.1
- **Platform**: macOS arm64 (Apple Silicon)

## Repo layout

Key files in `/Users/tyler/code/sandbox/duckdb/ducklake`:

```
deno.json                                    # root config: imports, supabase task, nodeModulesDir
.mise.toml                                   # pins Deno to 2.1.4
test_duckdb.ts                               # standalone Deno test script (works)
supabase/config.toml                         # supabase local dev config
supabase/functions/hello-world/main.ts       # edge function using @duckdb/node-api (fails)
supabase/functions/hello-world/deno.json     # function-level deno config
supabase/functions/hello-world-wasm/main.ts  # edge function using @duckdb/duckdb-wasm
supabase/functions/hello-world-wasm/deno.json
```

## Steps to reproduce

1. Clone this repo and cd into it:
   ```
   cd /Users/tyler/code/sandbox/duckdb/ducklake
   ```
2. Ensure Deno 2.1.4 is active (pinned via `.mise.toml`):
   ```
   deno --version
   # deno 2.1.4
   ```
3. Install dependencies (allows supabase postinstall script to download its binary):
   ```
   deno install --allow-scripts=npm:supabase
   ```
4. Verify `@duckdb/node-api` works under plain Deno — run `test_duckdb.ts`:
   ```
   deno run --allow-all -c deno.json test_duckdb.ts
   # outputs: [ [ 42, "v1.5.1" ] ]
   ```
   This runs `/Users/tyler/code/sandbox/duckdb/ducklake/test_duckdb.ts` which creates an in-memory DuckDB instance, runs a query, and prints results. It succeeds.

5. Now serve the same logic via the Supabase edge runtime:
   ```
   deno task supabase functions serve hello-world --no-verify-jwt
   ```
   This starts the edge runtime and serves `/Users/tyler/code/sandbox/duckdb/ducklake/supabase/functions/hello-world/main.ts`.

6. Invoke the function:
   ```
   curl http://127.0.0.1:54321/functions/v1/hello-world
   ```
   This fails with the error below.

## Expected behavior

The function returns a JSON response with DuckDB query results, the same as when running under standalone Deno.

## Actual behavior

The edge runtime crashes the worker with:

```
runtime has escaped from the event loop unexpectedly: event loop error: Error: Not supported
    at Object.Module._extensions..node (node:module:801:9)
    at Module.load (node:module:662:32)
    at Function.Module._load (node:module:534:12)
    at Module.require (node:module:681:19)
    at require (node:module:818:16)
    at getNativeNodeBinding (file:///var/tmp/sb-compile-edge-runtime/node_modules/localhost/@duckdb/node-bindings/1.5.1-r.1/duckdb.js:11:20)
```

The root cause is `Module._extensions..node` throwing `"Not supported"` — the edge runtime does not allow loading native `.node` addons. `@duckdb/node-bindings` ships a platform-specific `.node` shared library that it loads via `require()`, which works in Node and standalone Deno (via Node compat) but is blocked in the edge runtime's sandboxed worker environment.

## Analysis

- **Standalone Deno 2.1.4**: Supports native Node addons via its Node compatibility layer — `@duckdb/node-bindings` loads fine.
- **Supabase edge-runtime 1.73.3**: Uses Deno 2.1.4 internally but runs user code in a sandboxed `UserWorker`. The sandbox explicitly disables `Module._extensions..node`, preventing any native addon from loading.

This means any npm package that relies on native Node addons (N-API / node-gyp / prebuild) will hit this same wall on the edge runtime. This is not specific to DuckDB.

## Possible paths forward

1. **Allow native addons in edge runtime** — Enable `.node` loading in the worker sandbox (biggest lift, security implications).
2. **Support a WASM fallback** — DuckDB also ships `@duckdb/duckdb-wasm`, but that has its own issues on the edge runtime (Worker/importScripts not available in the sandbox).
3. **Document the limitation** — Clarify that npm packages with native addons are not supported on edge functions.

## Testing

### Automated integration test

```bash
RUSTUP_TOOLCHAIN=1.82.0 cargo test test_node_napi_duckdb -- --nocapture
```

### Interactive testing with the built binary

The main service builds worker paths relative to the working directory, so run the binary from `crates/base/` where `./test_cases/node-napi-duckdb` resolves correctly:

```bash
cd /Users/tyler/code/work/edge-runtime/crates/base

../../target/debug/edge-runtime start \
  --main-service ./test_cases/main \
  --port 9000
```

Then in another terminal:

```bash
curl http://localhost:9000/node-napi-duckdb
# {"rows":[[42,"vv1.5.1"]],"success":true}
```

### Interactive testing with the sandbox function

To serve the original function from the sandbox repo, symlink it into the `examples/` directory (the `examples/main` service routes `/hello-world` → `./examples/hello-world`):

```bash
ln -s /Users/tyler/code/sandbox/duckdb/ducklake/supabase/functions/hello-world \
      /Users/tyler/code/work/edge-runtime/examples/hello-world

./target/debug/edge-runtime start \
  --main-service ./examples/main \
  --port 9000

curl http://localhost:9000/hello-world
```

## Solution

Native Node addon support was enabled by making three changes:

### 1. Add `deno_napi` as a dependency (`Cargo.toml`, `crates/base/Cargo.toml`)

The `deno_napi` crate (v0.113.0, matching Deno 2.1.4) provides the `op_napi_open` op that loads `.node` shared libraries via `dlopen`. It was not previously a dependency of the edge runtime. It is also wired into the linker flags in `crates/base/build.rs` via `deno_napi::print_linker_flags` so that N-API symbols are exported correctly.

### 2. Register the `deno_napi` extension at runtime (`crates/base/src/runtime/mod.rs`)

`deno_napi::deno_napi::init_ops::<PermissionsContainer>()` was added to the extension list (placed before `ext_node`) so that `op_napi_open` is available when the worker boots.

### 3. Restore the `.node` handler in the Node polyfill (`ext/node/polyfills/01_require.js`)

The `Module._extensions[".node"]` handler was hardcoded to `throw new Error("Not supported")` with `op_napi_open` commented out. The throw was removed and the `op_napi_open(filename, globalThis, nodeGlobals.Buffer, reportError)` call was restored.

### 4. Grant `allow_ffi` to UserWorker (`crates/base/src/runtime/permissions.rs`)

`op_napi_open` internally calls `PermissionsContainer::check_ffi` before opening the library. UserWorker was missing `allow_ffi` from its default permissions (MainWorker and EventsWorker already had it), so the permission check was always failing. Adding `allow_ffi: Some(Default::default())` unblocks native addon loading for user functions.

## Resources

- **Node.js N-API documentation** — https://nodejs.org/api/n-api.html
  Background on what N-API is: a stable C ABI for building native Node addons that is version-independent from V8. This is the mechanism `@duckdb/node-bindings` (and most native npm packages) use to load `.node` shared libraries.

- **`deno_napi` crate (v0.113.0)** — https://crates.io/crates/deno_napi/0.113.0
  The Deno crate that implements `op_napi_open`, N-API symbol exports, and `print_linker_flags`. Version 0.113.0 is what Deno 2.1.4 ships with.

- **Deno 2.1.4 `runtime/worker.rs`** — https://github.com/denoland/deno/blob/v2.1.4/runtime/worker.rs
  Shows exactly how upstream Deno wires `deno_napi::deno_napi::init_ops_and_esm::<PermissionsContainer>()` into its worker extension list — the direct model for this fix.

- **Deno 2.1.4 `cli/build.rs`** — https://github.com/denoland/deno/blob/v2.1.4/cli/build.rs
  Shows the `deno_napi::print_linker_flags` call pattern used during the build to export N-API symbols on each platform (macOS `.def`, Linux, Windows).

- **Deno 2.1.4 `Cargo.lock`** — https://github.com/denoland/deno/blob/v2.1.4/Cargo.lock
  Used to confirm that `deno_napi = "0.113.0"` is the version compatible with `deno_core = "0.324.0"` and `deno_permissions = "0.42.0"` — the exact versions pinned in this repo.

## Implementation details — VFS extraction and N-API symbol export

After the initial four changes above, two additional runtime problems were discovered and fixed.

### Problem 1: Native binaries live in a virtual in-memory filesystem

The edge runtime packages all npm files (including `.node` binaries) into an in-memory virtual filesystem (VFS) backed by the eszip. File reads for JavaScript `require()` go through this VFS and work fine. But `op_napi_open` ultimately calls `dlopen`, which is a raw OS syscall that can only open files that exist on the real filesystem. The virtual path (e.g. `/var/tmp/sb-compile-<hash>/node_modules/localhost/@duckdb/node-bindings-darwin-arm64/1.5.1-r.1/duckdb.node`) does not exist on disk.

**Fix:** Added `op_require_extract_node_addon` to `ext/node/ops/require.rs`. This op:
1. Checks if the file already exists on real disk — if so, returns it as-is.
2. Reads the `.node` binary and any sibling `.dylib`/`.so`/`.dll` files from the VFS using `FileSystem::read_file_sync` and `read_dir_sync`.
3. Writes them all to a stable temp directory (`$TMPDIR/sb-napi/<hash-of-vfs-dir>/`) so they land together on the real filesystem. Sibling shared libraries must be co-located because `duckdb.node` has `@rpath = @loader_path`, meaning `libduckdb.dylib` is resolved relative to the loader's directory.
4. Returns the real filesystem path to the `.node` file.

The `.node` handler in `ext/node/polyfills/01_require.js` was updated to call `op_require_extract_node_addon(filename)` before passing the path to `op_napi_open`.

### Problem 2: N-API symbols not exported from test binaries

Native Node addons call N-API functions (`napi_create_function`, `napi_module_register`, etc.) that must be exported by the host process. The production `edge-runtime` binary gets these via `deno_napi::print_linker_flags("edge-runtime")` in `cli/build.rs`, which emits `-Wl,-exported_symbols_list,...` at link time.

Integration test binaries (compiled from `crates/base/tests/`) are separate executables that did not receive these linker flags. When `duckdb.node` loaded and tried to call `napi_module_register`, the symbol resolved to NULL, causing a SIGSEGV.

**Fix:** Added `emit_napi_linker_flags_for_tests()` to `crates/base/build.rs`. This function:
1. Determines the platform-specific N-API symbols export list filename (e.g. `generated_symbol_exports_list_macos.def`).
2. Locates the file by scanning the Cargo registry cache for the `deno_napi-*` source directory.
3. Emits `cargo:rustc-link-arg-tests=-Wl,-exported_symbols_list,<path>` (macOS) / the equivalent for Linux and Windows, which applies the export list to all integration test binaries in the crate.

### Files changed

| File | Change |
|------|--------|
| `Cargo.toml` | Added `deno_napi = "=0.113.0"` workspace dependency |
| `crates/base/Cargo.toml` | Added `deno_napi` to `[dependencies]` and `[build-dependencies]` |
| `cli/Cargo.toml` | Added `deno_napi` to `[build-dependencies]` |
| `cli/build.rs` | Added `deno_napi::print_linker_flags("edge-runtime")` |
| `crates/base/build.rs` | Added `emit_napi_linker_flags_for_tests()` to export N-API symbols from test binaries; also added `deno_napi` to snapshot extension list |
| `crates/base/src/runtime/mod.rs` | Added `deno_napi::deno_napi::init_ops::<PermissionsContainer>()` to runtime extension list |
| `crates/base/src/runtime/permissions.rs` | Added `allow_ffi: Some(Default::default())` to UserWorker permissions |
| `ext/node/ops/require.rs` | Added `op_require_extract_node_addon` op |
| `ext/node/lib.rs` | Registered `op_require_extract_node_addon` |
| `ext/node/polyfills/01_require.js` | Restored `op_napi_open` call; added `op_require_extract_node_addon` call before it |
| `crates/base/test_cases/node-napi-duckdb/` | New test case: `index.ts` runs a DuckDB query via `@duckdb/node-api` and returns JSON |
| `crates/base/tests/integration_tests.rs` | Added `test_node_napi_duckdb` integration test |
| `deno.json` | Added `node-napi-duckdb` test case to workspace |
