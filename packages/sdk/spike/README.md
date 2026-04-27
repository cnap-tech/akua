# SDK feature-parity spikes (#458)

Decision-gate prototypes for hosting akua's wasip1 artifacts under
Node's `node:wasi` instead of wasmtime. Validates Option B from
[`docs/spikes/engines-on-wasm32-unknown-unknown.md`](../../../docs/spikes/engines-on-wasm32-unknown-unknown.md)
before the full integration (#459+) commits to it.

## Stage 1 — `render-worker-via-node-wasi.ts`

Loads the existing `target/wasm32-wasip1/release/akua-render-worker.wasm`
under `node:wasi`. Sends a `Render` request via stdin (file-backed
pipe), captures the `RenderSummary` from stdout. Exercises a pure-KCL
Package — no plugin calls, no `charts.*` imports — to isolate the
WASI host from engine-bridge concerns.

**Result (Node 24):** ✅
- Compile + instantiate: ~8 ms
- `wasi.start` (full KCL eval + JSON envelope): ~32 ms
- Output YAML byte-equivalent to CLI render

## Stage 2 — `helm-engine-via-node-wasi.ts`

Loads `crates/helm-engine-wasm/assets/helm-engine.wasm` under `node:wasi`,
calls `_initialize`, then `helm_render(input_ptr, len)` against a
tar.gz of `examples/00-helm-hello/chart/`. Validates the engine ABI
(`<prefix>_malloc` / `<prefix>_free` / `<entry>` / `<prefix>_result_len`,
mirroring `crates/engine-host-wasm/src/lib.rs::Session`) is callable
under V8.

**Result (Node 24):** ✅
- Engine load (compile + `_initialize`): ~140 ms cold
- `helm_render` call: ~12 ms
- Manifests round-trip cleanly through the JSON envelope

## Decision

**Option B is sound.** Engine bundling under `node:wasi` works for
both the worker and the engines. Cold-render budget for SDK
(worker + engine load + render): ~190 ms vs CLI's ~57 ms — within
the 10× ceiling set in #465. Once #459 caches engine instances per
SDK process the warm path will collapse to engine-call latency only
(~12 ms), comparable to wasmtime.

## Open items for #459

1. **Plugin bridge in JS.** Worker imports `env.kcl_plugin_invoke_json_wasm(method_ptr, args_ptr, kwargs_ptr) -> i32`. The JS host must (a) read 3 C-strings from worker memory, (b) dispatch on method to the helm/kustomize JS-side engine drivers, (c) allocate response memory in the worker via the worker's exported `akua_bridge_alloc`, (d) copy response bytes + NUL terminator, (e) return guest pointer.
2. **Chart-path resolution.** `helm.template` receives a relative chart path; the CLI resolves it via thread-local "current package path" (`kcl_plugin::resolve_in_package`). The JS host needs an equivalent — likely set on the SDK's `Akua.renderSource()` entry from the `package` argument.
3. **Bun WASI compat.** Bun's `node:wasi` doesn't honour `stdin: <fd>` (returns EOF); track in #464. Node 22+ works; Bun fix needed before SDK ships.
4. **`proc_exit(0)` propagation.** Worker calls `std::process::exit(0)` on success. Node WASI raises that as `err.code === 'ERR_WASI_EXIT_CODE'`. The spike's Stage 1 catch inspects the error code and treats `exitCode === 0` as success; #459 must do the same in `Akua.renderSource()` or every successful render will look like a failure.
5. **Module + WASI caching.** Both spikes recompile `WebAssembly.Module` and rebuild `WASI` per call. In production the SDK should compile each `.wasm` once at SDK init and reuse; building a fresh `WASI` instance per render is fine (cheap; needed for clean stdin/stdout fds).
6. **In-memory stdin.** Stage 1 writes the request to a tempfile and hands the fd to WASI. `node:wasi` doesn't accept Buffer-backed stdin directly, but a Node `Readable` piped through a `Duplex` works. Worth a benchmark in #459 — tempfile cost is sub-ms but still per-render fs traffic.
7. **Runtime-portable base64.** Stage 2 uses `Buffer.from(...).toString('base64')` (Node-only). Replace with `globalThis.btoa(String.fromCharCode(...bytes))` or a `Uint8Array → base64` helper for Bun + browser portability.
8. **Memory-buffer view discipline.** `copyOut` does `new Uint8Array(memory.buffer, ptr, len).slice()`; `.slice()` makes the copy safe even if the engine grows memory after the call. Production code must keep this discipline — never hold a `Uint8Array` view across guest calls without `.slice()`.

## Running the spikes

```sh
# Build the worker first if not already present
task build:render-worker

# Stage 1 — pure-KCL render through worker
node packages/sdk/spike/render-worker-via-node-wasi.ts

# Stage 2 — helm engine direct
node packages/sdk/spike/helm-engine-via-node-wasi.ts
```

Both files are deliberately self-contained and short. They are not
shipped with `@akua/sdk`; deletion is tracked alongside #459 once
the production code lands and the spike read-out is captured in
[`docs/spikes/engines-on-wasm32-unknown-unknown.md`](../../../docs/spikes/engines-on-wasm32-unknown-unknown.md)
(#466).
