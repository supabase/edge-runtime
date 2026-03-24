# Upgrade Deno runtime from v2.1.4 to v2.7.7

## Summary

Upgrade the Deno runtime dependency from v2.1.4 (deno_core 0.324) to v2.7.7 (deno_core 0.393) to stay current with security fixes, performance improvements, and ecosystem compatibility.

## Motivation

- Deno v2.1.4 is 6+ months behind the latest stable release
- Security patches in V8 and deno_core are not reaching production
- Newer npm/Node.js compatibility features are unavailable
- Ecosystem drift increases upgrade difficulty over time

## Scope

This is a large upgrade affecting the entire runtime. The version gap spans:

| Component | Current | Target | Jump |
|-----------|---------|--------|------|
| deno_core | 0.324 | 0.393 | +69 versions |
| V8 (rusty_v8) | v130.0.7 | v146.8.0 | +16 versions |
| eszip | 0.80 | 0.118 | +38 versions |
| Rust toolchain | 1.82 | 1.92 | +10 versions |
| Rust edition | 2021 | 2024 | breaking |
| ~30 deno_* crates | various | various | all need bumps |

## Pre-requisites

Before implementation can begin, these fork updates must be completed:

### 1. Update supabase/rusty_v8 fork (CRITICAL)

Create a new tag for v146.8.0 with the Locker API patch reapplied.

The Locker API (`src/locker.rs`, ~107 lines) is fundamental to multi-tenant isolation. It must be ported to the new V8 version. The V8 cppgc thread-safety backports may already be in V8 v146 upstream — verify before porting.

**Patches to carry forward:**
- `src/locker.rs` — v8::Locker wrapper (~107 lines)
- `src/isolate.rs` — make `init_scope_root`/`dispose_scope_root` public
- `src/binding.cc` — C++ glue for Locker
- `OwnedIsolate::drop` — skip exit/dispose_scope_root when Locker active
- `src/template.rs` — null deref fix (+1/-3 lines)

**Repo:** https://github.com/supabase/rusty_v8
**Current tag:** `v130.0.7`
**Target tag:** `v146.8.0`

### 2. Update supabase/deno_core fork (may be eliminable)

The current `324-supabase` branch carries 4 small patches (~40 lines total). Check if they've been upstreamed into deno_core 0.393:

- PollState::Parked race fix in `core/inspector.rs`
- `extension!` macro multiple objects fix
- `#[varargs]` support in op2 object wrap
- ModEvaluate leak fix in `core/modules/map.rs`

Note: The `331-supabase` branch only carries the PollState fix, suggesting the others may have been upstreamed by 0.331. **If all 4 are upstream in 0.393, the fork can be eliminated entirely.**

**Repo:** https://github.com/supabase/deno_core
**Current branch:** `324-supabase`
**Target:** `393-supabase` or upstream `0.393.0`

### 3. Update supabase/eszip fork (may be eliminable)

The current fork makes ~20 items `pub` that are `pub(crate)` upstream, plus pins `import_map` to 0.20.1. Check if upstream eszip 0.118 has made these items public.

**Patches to carry forward (if still needed):**
- `src/v2.rs` — ~23 lines changing `pub(crate)` to `pub`
- `Cargo.toml` — `import_map` version pin

**Repo:** https://github.com/supabase/eszip
**Current branch:** `fix-pub-vis-0-80-1`
**Target:** new branch for 0.118 or upstream `0.118.0`

## Implementation plan

### Phase 1: Rust toolchain upgrade

**Estimated effort:** 1 day

Update toolchain and edition independently of Deno changes.

- [ ] Update `rust-toolchain.toml` to channel `"1.92.0"`
- [ ] Update `Cargo.toml` workspace edition to `"2024"`
- [ ] Update each `vendor/*/Cargo.toml` edition
- [ ] Update Dockerfile rust base image
- [ ] Fix edition 2024 breaking changes:
  - `unsafe_op_in_unsafe_fn` is deny-by-default — add `unsafe` blocks inside unsafe fns (`crates/base/src/runtime/mod.rs`, `ext/runtime/external_memory.rs`)
  - `impl Trait` lifetime capture rule changes
  - `gen` is reserved keyword
  - `tail_expr_drop_order` changes may affect scopeguard usage
- [ ] Verify: `cargo check` passes, `cargo test` passes

### Phase 2: Intermediate upgrade to deno_core 0.331

**Estimated effort:** 3-5 days

Use the existing `331-supabase` fork branch as a stepping stone.

- [ ] Update `Cargo.toml` deno_core to `"0.331.0"`, patch branch to `"331-supabase"`
- [ ] Update all correlated `deno_*` crate versions to match 0.331 compatibility
- [ ] Determine matching V8 version; update rusty_v8 fork tag if needed
- [ ] Re-vendor `vendor/deno_fetch/` at matching version, reapply rate limiter patch in `26_fetch.js`
- [ ] Re-vendor `vendor/deno_http/` at matching version
- [ ] Re-vendor `vendor/deno_telemetry/` at matching version, reapply `base_rt` integration
- [ ] Re-vendor `vendor/deno_unsync/` if version changed
- [ ] Fix compilation errors (order: `deno/` → `ext/` → `crates/deno_facade/` → `crates/base/`)
- [ ] Cherry-pick any supabase patches not on `331-supabase` if still needed
- [ ] Verify: `cargo check`, `cargo test`, integration tests pass

### Phase 3: Error system migration

**Estimated effort:** 5-8 days (highest risk phase)

The new deno_core uses `deno_error` crate (v0.7.x) with `#[derive(JsError)]` proc macro. Errors derive their JS class name at compile time instead of the runtime `get_error_class_fn` classification function. This is the single largest breaking change.

- [ ] Add `deno_error` as workspace dependency
- [ ] Delete `deno/runtime/errors.rs` (1,936 lines) — entirely replaced by derive macros
- [ ] Delete `deno/errors.rs` (121 lines)
- [ ] Remove `get_error_class_fn` from RuntimeOptions in `crates/base/src/runtime/mod.rs:883`
- [ ] Remove `import_meta_resolve_callback` from RuntimeOptions in `crates/base/src/runtime/mod.rs:888` (also removed upstream)
- [ ] Replace all `deno_core::error::custom_error("ClassName", msg)` calls (31 sites, 17 files) with typed errors deriving `JsError`
- [ ] Replace all `deno_core::error::type_error(msg)` calls with new equivalents
- [ ] Update ops returning `Result<_, AnyError>` to use specific error types where required
- [ ] Port `deno/` crate files to match upstream Deno CLI at target version:
  - `deno/graph_util.rs` (1,251 lines) — module graph API changes
  - `deno/resolver.rs` (488 lines) — module resolution changes
  - `deno/npm/managed/mod.rs` (710 lines) — NPM resolution changes
  - `deno/file_fetcher.rs` (822 lines) — HTTP fetch/cache changes
  - `deno/emit.rs` — transpiler option changes
  - `deno/cache/*.rs` (13 files) — cache schema changes
  - `deno/args/` (5 files) — config loading changes
- [ ] Update `ext/node/` (40 Rust files + 188 TypeScript polyfills) against upstream
- [ ] Verify: `cargo check`, `cargo test`, snapshot build succeeds, integration tests pass

### Phase 4: Final target — deno_core 0.393, V8 v146, eszip 0.118

**Estimated effort:** 3-5 days

- [ ] Create/update all 3 supabase fork branches (see pre-requisites above)
- [ ] Update all ~30 `deno_*` crate version pins in `Cargo.toml` to final v2.7.7 versions:

  | Crate | Target Version |
  |-------|---------------|
  | deno_core | 0.393.0 |
  | deno_ast | 0.53.1 |
  | deno_graph | 0.107.1 |
  | deno_config | 0.90.0 |
  | deno_npm | 0.52.0 |
  | deno_permissions | 0.99.0 |
  | deno_lockfile | 0.42.0 |
  | deno_fetch | 0.264.0 |
  | deno_http | 0.238.0 |
  | deno_net | 0.232.0 |
  | deno_tls | 0.227.0 |
  | deno_web | 0.271.0 |
  | deno_url | 0.222.0 |
  | deno_crypto | 0.254.0 |
  | deno_fs | 0.150.0 |
  | deno_io | 0.150.0 |
  | deno_websocket | 0.245.0 |
  | deno_telemetry | 0.62.0 |
  | eszip | 0.118.0 |
  | deno_cache | 0.173.0 |
  | deno_npm_cache | 0.59.0 |
  | deno_resolver | 0.71.0 |
  | node_resolver | 0.78.0 |
  | deno_package_json | 0.42.0 |
  | import_map | 0.25.0 |
  | (all others) | (match v2.7.7 Cargo.lock) |

- [ ] Re-vendor all 4 crates at final versions + reapply patches
- [ ] Update `crates/base/build.rs` — snapshot extension init signatures, permission trait methods
- [ ] Update `crates/base/src/runtime/mod.rs` — new RuntimeOptions fields
- [ ] Fix all remaining compilation errors
- [ ] Verify: full `cargo check`, `cargo test`, integration tests

### Phase 5: Validation and hardening

**Estimated effort:** 3-5 days

- [ ] Run full test suite (115 unit tests + integration tests)
- [ ] Run `cargo llvm-cov` — verify coverage >= 53% baseline
- [ ] Manual testing:
  - Simple edge functions (hello world, fetch)
  - NPM dependency functions (express, hono)
  - Node.js polyfill functions (fs, http, crypto)
  - WebSocket upgrade functions
  - OTEL telemetry export
  - Rate limiting (deno_fetch patch)
  - Memory limits + CPU limits
  - Inspector/debugger
  - Multi-tenant concurrent execution (v8::Locker correctness)
- [ ] Performance comparison vs v2.1.4:
  - Cold start time
  - Request throughput
  - Memory usage under load
- [ ] `cargo clippy` + `cargo fmt` cleanup
- [ ] Docker build + container smoke test
- [ ] Update CI if needed

## Key files affected

| File | Lines | Impact |
|------|-------|--------|
| `Cargo.toml` | 243 | All version pins, patches, vendor overrides |
| `crates/base/src/runtime/mod.rs` | 3,442 | JsRuntime, RuntimeOptions, v8::Locker, event loop |
| `crates/base/build.rs` | 337 | Snapshot generator — all extensions + permissions |
| `deno/runtime/errors.rs` | 1,936 | DELETE — replaced by deno_error derive macros |
| `deno/errors.rs` | 121 | DELETE |
| `deno/graph_util.rs` | 1,251 | Port upstream changes |
| `deno/resolver.rs` | 488 | Port upstream changes |
| `deno/lib.rs` | 569 | DenoOptions/DenoOptionsBuilder updates |
| `deno/file_fetcher.rs` | 822 | Port upstream changes |
| `deno/npm/` | ~1,800 | Port upstream changes |
| `deno/cache/` | ~2,000 | Port upstream changes |
| `ext/node/lib.rs` | 904 | Extension + 40+ ops |
| `ext/node/ops/*.rs` | ~28,000 | 40 files of node compat ops |
| `ext/runtime/lib.rs` | 621 | Extension + ops |
| `vendor/deno_fetch/26_fetch.js` | ~500 | Rate limiter patch |
| `vendor/deno_telemetry/lib.rs` | 2,179 | base_rt integration |
| All files using `deno_core::error::*` | 17 files | Error API migration |
| All files using `deno_core::unsync` | 35+ files | Possible API changes |

## Risks

| Risk | Severity | Mitigation |
|------|----------|------------|
| Error system migration breaks all error handling | High | Phase 3 is dedicated to this; test each error path |
| rusty_v8 Locker patch doesn't port cleanly | High | This is the multi-tenant foundation; allocate extra time |
| `deno/` crate port introduces subtle behavior changes | Medium | Diff upstream carefully; test module loading paths |
| Edition 2024 unsafe/lifetime changes | Medium | Can temporarily pin edition 2021 if blocking |
| Vendored crate API drift too large to re-patch | Medium | Keep patches minimal; consider upstreaming rate limiter |

## Estimated total effort

**17-27 working days** across all phases, assuming pre-requisite fork updates are done.

## Test coverage baseline

The `FUNC/code-coverage` branch adds 115 new unit tests across 8 crates covering critical Deno API surface areas. These tests serve as a regression safety net for the upgrade:

- `ext/runtime/` — 28 tests (rate limiter, metrics, promises, cert store)
- `ext/workers/` — 16 tests (permissions, worker context, lifecycle)
- `crates/deno_facade/` — 26 tests (metadata, module loader, eszip, utilities)
- `crates/base/` — 28 tests (units, json merge, termination tokens)
- `crates/http_utils/` — 7 tests (upgrade detection, status emission)
- `crates/base_rt/` — 5 tests (runtime state flags)
- `crates/base_mem_check/` — 5 tests (heap statistics, serialization)

Current overall coverage: 53.37% (19,549 / 36,630 lines).
