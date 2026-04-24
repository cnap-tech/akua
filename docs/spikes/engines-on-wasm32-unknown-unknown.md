# Spike: helm/kustomize engines on `wasm32-unknown-unknown` (browser SDK)

## Context

Phase 4 shipped `akua-wasm` compiling the KCL render path to `wasm32-unknown-unknown`. `@akua/sdk` v0.0.0 consumes the Node build; `Akua.renderSource()` renders pure-KCL Packages in-process.

The remaining Phase 4B gap is **engine callouts from the browser**: `helm.template(...)` and `kustomize.build(...)` are implemented as Go engines compiled to `wasm32-wasip1`, hosted inside the CLI's wasmtime Engine via the `engine-host-wasm` crate. That shape works on the CLI (`akua render`) and on `@akua/sdk` for Node (bundled CLI binary — though the SDK today wires only `renderSource`, not the helm/kustomize callouts). It does **not** work in the browser: there's no wasmtime, no WASI host, and the `env::kcl_plugin_invoke_json_wasm` bridge has no one to answer it.

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

### C. Punt browser to v0.2.0. Ship `@akua/sdk` Node-only for v0.1.0.

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

**C. Ship `@akua/sdk` v0.1.0 Node-only. Punt browser to v0.2.0.**

Reasoning:

1. **The v0.1.0 security-invariant promise is already kept by what ships.** Phase 4's sandbox + Phase 4B's Node build + adversarial suite are the load-bearing pieces. Browser is a deployment-target question, not an invariant question.
2. **The agent-first positioning doesn't need browser.** Agents (Claude Code, Cursor, Codex, etc.) run on Linux sandboxes with Node or bun. That's the user. Browser matters for `akua.dev/playground` and embedded audit UIs — important, not v0.1.0-critical.
3. **Both engineering candidates are uncertain + multi-week.** Option A risks fork divergence; option B risks perf blowout. Either delays v0.1.0 by >2 weeks with real risk of further slipping.
4. **The punt is additive.** `package.json` conditional exports are already structured for a `"browser"` condition; adding it in v0.2.0 is a minor version bump, not a breaking change. Consumers who import `@akua/sdk` today keep working.

## What lands for v0.1.0 under this decision

- `@akua/sdk` ships to JSR with Node-loadable WASM bundle (`packages/sdk/wasm/nodejs/`) only.
- `package.json` / `jsr.json` `exports` condition on `"node"` + `"default"` (both resolve to the same ESM module for now — `"default"` exists so Deno + Bun still resolve something).
- No `packages/sdk/wasm/browser/` directory in v0.1.0 publish. Dry-run verifies.
- Release notes explicitly name "Node 20+" (+ Deno + Bun via Node-compat runtime resolution) as the v0.1.0 target.
- `docs/sdk.md`'s "Browser" section stays, framed as "v0.2.0" / "target-state". Not a regression — it was already framed that way after the docs sweep.
- Roadmap's Phase 4B exit gate tightened to Node + Deno + Bun only for v0.1.0; browser moves to Phase 4B's v0.2.0 completion.

## What v0.2.0 needs to revisit

- Re-evaluate A vs B with another 6 months of tooling maturation. In particular: wasmer-js 2026 releases, wasm-component-model browser support, WASI-preview-2 browser polyfills.
- If neither matures, commit to A and budget 3 weeks for the Go-engine recompile + bridge re-implementation.
- Ship `packages/sdk/wasm/browser/` + the conditional-exports `"browser"` key.

## References

- [docs/roadmap.md § Phase 4B](../roadmap.md#phase-4b--akua-wasm-for-jsr-delivery-blocks-v010) — parent phase.
- [docs/spikes/kcl-wasm-feasibility.md](kcl-wasm-feasibility.md) — KCL on both wasm32 targets.
- [docs/spikes/wasmtime-multi-engine.md](wasmtime-multi-engine.md) — Engine/Store architecture the bridge depends on.
