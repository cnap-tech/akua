# Spike: helm/kustomize engines on `wasm32-unknown-unknown` (browser SDK)

## Context

Phase 4 shipped `akua-wasm` compiling the KCL render path to `wasm32-unknown-unknown`. `@akua-dev/sdk` v0.0.0 consumes the Node build; `Akua.renderSource()` renders pure-KCL Packages in-process.

The remaining Phase 4B gap is **engine callouts from the browser**: `helm.template(...)` and `kustomize.build(...)` are implemented as Go engines compiled to `wasm32-wasip1`, hosted inside the CLI's wasmtime Engine via the `engine-host-wasm` crate. That shape works on the CLI (`akua render`) and on `@akua-dev/sdk` for Node (bundled CLI binary — though the SDK today wires only `renderSource`, not the helm/kustomize callouts). It does **not** work in the browser: there's no wasmtime, no WASI host, and the `env::kcl_plugin_invoke_json_wasm` bridge has no one to answer it.

v0.1.0 blocks on deciding what to do. This doc enumerates the candidates and records the call.

## Candidates

### A. Recompile helm + kustomize Go engines to `wasm32-unknown-unknown`

Go's wasm targets are `GOOS=wasip1` (what we ship today) or `GOOS=js` (what the browser uses — it emits `wasm32-unknown-unknown` bytes that rely on `syscall/js` for stdio / time / etc.).

- **Pros:** native JS loading; no extra runtime dep; same binary size ballpark.
- **Cons:**
  - `GOOS=js` ties the engine to a browser environment via `syscall/js`. Node would need a `syscall/js` shim (Node has one in the Go distribution, but it's load-bearing and brittle).
  - Our helm fork strips client-go specifically to get the wasip1 build small; the patches would need re-auditing against `GOOS=js`.
  - Kustomize's `wasip1` build already needs careful surgery; `GOOS=js` is a separate adventure.
  - The plugin bridge protocol (`kcl_plugin_invoke_json_wasm` + akua-core's `resolve_in_package` path guard) assumes a single wasmtime Engine. Browser JS has no wasmtime. We'd need to re-implement the bridge as pure JS.

Estimate: 2-3 weeks of Go-engine plumbing + bridge re-implementation, with real risk of regressions in the CLI path if the forks diverge.

### B. Run the existing `wasm32-wasip1` modules under a JS WASI polyfill (wasmer-js / wasmtime's JS bindings / `@bjorn3/browser_wasi_shim`)

Keep the existing `.wasm` artifacts; load them in the browser through a WASI-in-JS runtime.

- **Pros:** one set of engine builds; no fork divergence between CLI and SDK; bridge protocol can be mirrored in JS without changing the engines.
- **Cons:**
  - `wasmer-js` is ~2 MB minified and not maintained for 2026-era wasmtime features (epoch interruption, StoreLimits).
  - `@bjorn3/browser_wasi_shim` is lighter but experimental and doesn't support the preopen model our render worker depends on.
  - Nested wasm-in-JS interpretation is slow — benchmarks in other projects show 5-10× native; we'd blow the sub-100ms dev-loop budget badly.
  - The plugin bridge still needs a JS implementation that imitates wasmtime's host-function wiring; not trivial.

Estimate: 1-2 weeks of JS runtime wrangling + perf work. Uncertain whether the perf budget is reachable.

### C. Punt browser to v0.2.0. Ship `@akua-dev/sdk` Node-only for v0.1.0.

- **Pros:**
  - v0.1.0 ships this quarter, not next.
  - Node SDK stays the v0.1.0 headline: pure KCL, helm, kustomize, the lockfile/signing verbs — all working in-process.
  - `package.json` conditional exports are already in place; adding a `"browser"` condition in a minor release is non-breaking.
  - Doesn't compromise the "sandboxed by default" invariant — the sandbox holds for every environment where the SDK ships.
- **Cons:**
  - `akua.dev/playground` doesn't ship at v0.1.0.
  - "No CLI install" is only half-true for browsers.
  - Marketing narrative trims back: "JSR-distributable in-process SDK" instead of "runs in the browser too."

Estimate: zero engineering; just docs + release-note framing.

## Decision

**C. Ship `@akua-dev/sdk` v0.1.0 Node-only. Punt browser to v0.2.0.**

Reasoning:

1. **The v0.1.0 security-invariant promise is already kept by what ships.** Phase 4's sandbox + Phase 4B's Node build + adversarial suite are the load-bearing pieces. Browser is a deployment-target question, not an invariant question.
2. **The agent-first positioning doesn't need browser.** Agents (Claude Code, Cursor, Codex, etc.) run on Linux sandboxes with Node or bun. That's the user. Browser matters for `akua.dev/playground` and embedded audit UIs — important, not v0.1.0-critical.
3. **Both engineering candidates are uncertain + multi-week.** Option A risks fork divergence; option B risks perf blowout. Either delays v0.1.0 by >2 weeks with real risk of further slipping.
4. **The punt is additive.** `package.json` conditional exports are already structured for a `"browser"` condition; adding it in v0.2.0 is a minor version bump, not a breaking change. Consumers who import `@akua-dev/sdk` today keep working.

## What landed for v0.1.x under this decision

- `@akua-dev/sdk` shipped Node + Bun + Deno (anything that loads Node-API addons), via the **napi-rs path** rather than the wasm32-unknown-unknown JSR bundle the original spike framed (#468–#472):
  - `@akua-dev/native` per-platform `.node` binary embeds the same wasmtime + helm-engine + kustomize-engine the CLI uses.
  - Same sandbox, same engines, same render orchestrator — no fork.
  - Distributed via npm `optionalDependencies` (one binary downloads per host).
- The earlier `packages/sdk/wasm/nodejs/` bundle is still produced for the pure-KCL fast path (no engine callouts), but the napi addon is the SDK's primary transport for the verbs that need OCI fetch / cosign verify / helm / kustomize.
- The JSR distribution channel was retired entirely — JSR's 20 MB single-file/total-package cap is incompatible with the napi binary. See the rename note in `CHANGELOG.md` under `@akua-dev/sdk 0.6.0`.

## What v0.2.x needs for browser

The napi route doesn't extend to the browser (no Node-API there). The browser path therefore still wants candidate **A** or **B** from above, with the additional constraint that the in-process render must reuse the existing wasmtime-host plugin protocol or replace it with a pure-JS equivalent. Status as of 2026-04:

- **A (`GOOS=js` engine recompile):** still uncertain. wasm-component-model + WASI-preview-2 browser polyfills landing piecemeal in 2026 reduce the bridge re-implementation surface but don't eliminate it. Budget 2-3 weeks when prioritized.
- **B (WASI-in-JS polyfill on existing wasip1 modules):** `@bjorn3/browser_wasi_shim` gained preopen support during 2026; wasmer-js stays heavy. Worth re-spiking when v0.2.x browser becomes a hard requirement.
- **C (defer further):** unchanged option. Browser still doesn't gate the agent-first usage akua targets.

When the work is taken up, this doc becomes the running state — append decisions inline, don't re-fork.

## References

- [docs/roadmap.md § Phase 4B](../roadmap.md#phase-4b--akua-wasm-for-jsr-delivery-blocks-v010) — parent phase (note: roadmap section title still references JSR; the channel is npm now, but the phase numbering is stable).
- [docs/spikes/kcl-wasm-feasibility.md](kcl-wasm-feasibility.md) — KCL on both wasm32 targets.
- [docs/spikes/wasmtime-multi-engine.md](wasmtime-multi-engine.md) — Engine/Store architecture the bridge depends on.
- `crates/akua-napi/` — the v0.6.0 napi-rs implementation that replaces the wasm32-unknown-unknown approach for Node/Bun/Deno consumers.
