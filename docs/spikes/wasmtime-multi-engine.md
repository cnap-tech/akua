# Spike: wasmtime multi-Engine vs single-Engine

Date: 2026-04-24. Task: #419. Blocks: Phase 4 (sandboxed render end-to-end).

## Question

Our plugin bridge sits between the render worker (Engine A, Store A) and the engine plugins (helm, kustomize — Engine B, Store B). When an untrusted Package inside Store A calls `helm.template(...)`, we need helm to run somewhere. Is it safe and supported to run it in a **second wasmtime Engine**, where Store A pauses mid-host-call while Store B executes?

The alternative: one `Engine`, one `Linker`, many `Store`s — the render worker and every plugin share the same Engine, with per-invocation Stores providing isolation.

## Method

1. Delegated a research pass against wasmtime's docs + source + test suite + GitHub discussions.
2. Wrote a verification test (`crates/akua-cli/tests/sandbox_nested_wasmtime.rs`) exercising helm `render_dir` through the plugin bridge end-to-end.

## Findings

### Docs + source say: one Engine per process

From wasmtime's own root-level documentation: *"typically there's one [`Engine`] per process."* The contributing-architecture notes call the Engine a *"global compilation context"* and the *"root context."*

Process-global state that's initialized **per-Engine construction**, not per-process:

- **Trap handlers** and signal handlers (`init_traps` in `traphandlers/signals.rs`) register one handler the first time any Engine is built. They dispatch via thread-local state that tracks "which Store is currently executing on this thread." With multiple Engines, the dispatch works *as long as only one Store per thread is active at any time* — but running two Engine's Stores on the same thread simultaneously (our nested pattern) is not in wasmtime's test matrix.
- **Epoch interruption counter** is tied to an Engine. Multiple Engines = multiple counters = multiple tickers.
- **Compat-hash** on precompiled `.cwasm` is per-Engine-Config. Different Configs → different `.cwasm` artefacts.

Performance + resource cost of duplicate Engines:

- Separate Cranelift compiler instances, type registries, instance allocators, GC runtimes.
- No shared JIT cache. Same module loaded in two Engines compiles twice.
- No shared type interning across Engines (indices aren't portable).

### No examples of nested-Engine execution in wasmtime

Searched:

- `examples/` — the WASIP2 plugins example shows multiple plugin components in **one Engine with one Linker**, each in its own `Store`. Canonical.
- `crates/wasmtime/tests/all/` — no tests with Engine-A-host-fn-calls-into-Engine-B.
- GitHub issues/discussions — no precedent; a couple of references to "multiple engines" land on "consolidate to one."

### Verification: our first attempt (separate Engines) failed

Without changes, the research-flagged failure mode fired:

- Helm wasm trapped on entry to `_initialize` (`<unknown>!<wasm function 1869>`) when called from inside the worker's bridge host function.
- Stderr empty — the trap came from wasm instructions, not a Rust panic.
- Likely cause: TLS for "currently executing Store" clobbered, or signal handler misrouting between the two Engine instances.

### Refactor + verification: one Engine works

- New `engine_host_wasm::shared_config()` + `shared_engine()` (OnceLock singleton). One Config: `wasm_exceptions` + `epoch_interruption` (no fuel — would force every engine Store to `set_fuel` before every call).
- `engine_host_wasm::Session::init` uses `shared_engine()` instead of constructing a fresh Engine.
- `akua_cli::render_worker::RenderHost` holds `&'static Engine` borrow into `shared_engine()`.
- `akua-cli`'s `build.rs` routes through `shared_config()` so the AOT `.cwasm` matches the runtime Config hash automatically.
- Every engine-plugin Store opts out of the epoch ticker via `set_epoch_deadline(u64::MAX)` — the host-Rust caller above them owns whole-call timeouts.

End-to-end verification test (`helm_template_through_plugin_bridge_across_engines`):

- KCL source inside the worker's Store: `import kcl_plugin.helm; _manifests = helm.template({...})`.
- Worker calls `env::kcl_plugin_invoke_json_wasm`, pauses.
- Bridge (host Rust) reads the JSON args, dispatches to akua-core's registered `helm.template` handler.
- Handler calls `helm_engine_wasm::render_dir` — spins up a **new Store of the shared Engine**, instantiates helm, runs `_initialize` + render.
- Manifest bytes → handler → bridge → guest memory via `akua_bridge_alloc` → KCL resumes in Store A.
- Final YAML contains the marker string the KCL code passed as the helm `values.greeting`.

**Passes.** Two Stores of one Engine, one paused and one running, single OS thread, synchronous. This is the documented pattern.

## Subtle bug fix: epoch ticker + engine Stores

The render worker spawns a background thread that calls `engine.increment_epoch()` every 100 ms. With the unified Engine, that tick applies to **every Store on the Engine** — including plugin Stores. Default `epoch_deadline = 0` means "trap on first check past epoch 0." So the moment the ticker fired, every plugin Store trapped immediately.

Fix: `engine_host_wasm::Session::init` calls `store.set_epoch_deadline(u64::MAX)` on every plugin Store. The render worker's Store gets the real deadline (30 ticks = 3 s) separately.

Before the fix, even **direct** (non-nested) helm calls trapped on the same backtrace. That was the tell — it wasn't the nested pattern at all, it was the shared epoch state.

## Recommendation — shipped

One Engine, one Linker of host imports, many per-invocation Stores. All engine plugins share the Engine via `engine_host_wasm::shared_engine()`. See the updated Execution Model section in [docs/security-model.md](../security-model.md#one-engine-many-stores--with-a-plugin-bridge).

## Future: if the sandbox ever needs genuine isolation between plugins

Today, sharing an Engine means all plugins trust the same Cranelift settings, the same signal-handler setup, etc. If some plugin class ever needs cryptographic isolation from other plugins — e.g., a third-party Rego bundle whose author is not the Package author — we'd spawn a separate process (not a separate Engine) and IPC to it. Process isolation is the canonical wasmtime answer to "adversary + adversary in the same process" per their docs.

## Artefacts

- [crates/engine-host-wasm/src/lib.rs](../../crates/engine-host-wasm/src/lib.rs): `shared_config`, `shared_engine`, the `epoch_deadline = u64::MAX` fix.
- [crates/akua-cli/src/render_worker.rs](../../crates/akua-cli/src/render_worker.rs): `RenderHost` + plugin bridge.
- [crates/akua-cli/tests/sandbox_nested_wasmtime.rs](../../crates/akua-cli/tests/sandbox_nested_wasmtime.rs): the verification test. `#[ignore]` by default; run with `cargo test -p akua-cli --test sandbox_nested_wasmtime -- --include-ignored`.
