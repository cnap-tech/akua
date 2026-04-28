# Spike: KCL → wasm32 feasibility

Date: 2026-04-24. Task: #408. Blocks: Phase 4 + Phase 4B + v0.1.0.

## Question

Can `kcl-lang` compile to WebAssembly targets without maintaining a fork? Specifically:

1. **wasm32-wasip1** (Phase 4 — CLI wasmtime-hosted render)
2. **wasm32-unknown-unknown** (Phase 4B — `@akua-dev/sdk` via JSR, browser + JS runtimes)

## Method

Minimal external harness at `/tmp/kcl-wasm-spike/`: `kcl-lang` as the only dep, one `pub fn smoke()` that constructs `kcl_lang::API::default()`. Bypasses akua-core's existing `cfg(not(target_arch = "wasm32"))` gate so we see the raw upstream build behavior.

Built against `kcl-lang` commit `bde7439b` (git dep in Cargo.toml).

## Findings

### wasm32-wasip1 — green (runtime unblocked 2026-04-24)

```text
cargo build --target wasm32-wasip1
  Compiling kcl-lang v0.12.3
  Compiling kcl-wasm-spike v0.0.0
  Finished `dev` profile in 25.45s
```

Produces a 145 MB wasip1 binary (unoptimized debug). Compile is clean.

**Two runtime blockers hit during Phase 4 step 2 integration, both fixed same-day:**

1. **`kcl-driver::get_pkg_list`** at `crates/driver/src/lib.rs:328` calls `std::env::current_dir()` — unconditional panic on wasip1. Fixed via [cnap-tech/kcl@akua-wasm32](https://github.com/cnap-tech/kcl/tree/akua-wasm32) (pinned via workspace `[patch]`) + upstream PR [kcl-lang/kcl#2086](https://github.com/kcl-lang/kcl/pull/2086).

2. **`akua_core::stdlib::stdlib_root`** calls `std::env::temp_dir()` — also an unconditional panic on wasip1 (`sys/pal/wasip1/os.rs:119:5`, same error message as the kcl issue but a different call site). Fixed in-tree: skip the "akua" stdlib `ExternalPkg` on wasm32 (pure-KCL Packages work; stdlib-requiring Packages will land later when the plugin bridge forwards `akua.*` callouts from worker to host).

**Go signal for Phase 4 — confirmed.** End-to-end test shipped: `render_pure_kcl_returns_yaml_end_to_end` evaluates `x = 42\ngreeting = "hello"\n` inside the per-render wasmtime sandbox and recovers the top-level YAML with correct values. Sandbox resources (fuel / epoch / 256 MiB memory) active throughout. Test runs on every `cargo test -p akua-cli` invocation.

### wasm32-unknown-unknown — yellow, two fixable issues

```text
cargo build --target wasm32-unknown-unknown
error: to use `uuid` on `wasm32-unknown-unknown`, specify a source of randomness
       using one of the `js`, `rng-getrandom`, or `rng-rand` features
error[E0599]: no method named `to_file_path` found for reference `&lsp_types::Url`
error[E0599]: no function or associated item named `from_file_path` found for struct `lsp_types::Url`
  ... (8 such errors in kcl-language-server)
```

#### Issue 1: uuid missing random-source feature

Transitive dep via `kcl-primitives`. Fixed downstream by feature-unification in our Cargo.toml:

```toml
[target.'cfg(target_arch = "wasm32")'.dependencies]
uuid = { version = "1", features = ["js", "v4"] }
```

Three lines. Zero maintenance burden (feature union is a stable Cargo guarantee).

#### Issue 2: `kcl-language-server` uses fs-only `Url` helpers

`kcl-api` unconditionally pulls in `kcl-language-server`, which calls `lsp_types::Url::{to_file_path, from_file_path}` — methods gated behind the `file` feature of `url`/`lsp_types`, unavailable on wasm32-unknown-unknown. Eight call sites.

**LSP is not needed for render**, only for the language-server tooling surface. Three paths forward, ordered by preference:

1. **Upstream PR: make `kcl-language-server` an optional dep in `kcl-api`.** Default-on for tooling consumers (no behavior change), off for render-only embedders like akua. Cleanest fix; zero maintenance if upstream accepts.
2. **Upstream PR: `#[cfg(not(target_arch = "wasm32"))]` guards on the 8 call sites.** Mechanical, less API impact, almost certain to merge. Lets us keep using `kcl-api` as-is on wasm32.
3. **Fork as `cnap-tech/kcl` on branch `akua-wasm32` with either patch applied.** Fallback if upstream stalls. Low maintenance — both patches are additive cfg guards that won't conflict with upstream churn.

Estimated effort for path 1 or 2: 1-2 hours to open the PR, then upstream-response time (weeks). Estimated effort for path 3: ~4 hours to fork + apply + set up CI + write `cnap-tech/kcl` README pointing at the upstream branch we track.

**Go signal for Phase 4B.** Upstream work is non-trivial but bounded; fork is a tractable fallback.

## Other wasm32-unknown-unknown risks we didn't hit

Noting for posterity — none of these fired in the spike, but worth keeping in mind when engine work lands:

- **`rustc_span` path assertion** (`!p.to_string_lossy().ends_with('>')`) — trips when KCL filenames end in `>`, as noted in `akua_core::eval_source` docs. Orthogonal to target; not a wasm32 issue.
- **`std::fs` usage inside KCL itself** — kcl-loader reads source files via `std::fs`. On wasm32-unknown-unknown without a filesystem shim, this will surface at runtime if the Package has transitive imports. The SDK already ships its Package.k source as a string (no filesystem needed); for multi-file Packages the SDK will need to virtualize the FS (via a WASI shim or in-memory adapter). Deferred to the `akua-wasm` crate work.
- **Threading primitives** — wasm32-unknown-unknown has no threads. kcl-lang appears to run single-threaded; spike didn't exercise any parallel code path. Worth checking under real workloads during Phase 4B integration.

## Recommendation

1. **Phase 4 (wasip1) is unblocked.** Start the `akua-render-worker` scaffold immediately (#409).
2. **Phase 4B (unknown-unknown) is unblocked with two known workstreams:**
   - Land the uuid feature-unification in Cargo.toml when `akua-wasm` crate lands (trivial).
   - Open an upstream PR against `kcl-lang/kcl` to gate the LSP helpers; fall back to a minimal `cnap-tech/kcl` fork if that stalls beyond the v0.1.0 timeline.
3. **Update the roadmap:** Phase 4B's "same PRs Phase 4 needs (uuid features, kcl-language-server Url helpers) likely apply here too. Fork as cnap-tech/kcl on akua-wasm32 branch if upstream stalls." — rewrite to reflect the actual findings (Phase 4 needs neither; Phase 4B needs both, upstream PR preferred, fork is the fallback).

## Artifact

The spike harness at `/tmp/kcl-wasm-spike/` is throwaway. The real work starts at task #409 (scaffold `crates/akua-render-worker`) and the eventual `crates/akua-wasm` for Phase 4B.
