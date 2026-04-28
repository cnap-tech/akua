# Spike: nested wasm sandbox for `pkg.render`

**Status:** deferred to post-registry milestone. Ship a budget-cap header
in the meantime.

## Problem

`pkg.render` (the synchronous host plugin at
`crates/akua-core/src/pkg_render.rs`) re-enters KCL **in-process** when
an outer Package calls `pkg.render({path = "./inner"})`. The inner
render runs on the host process — same address space as the outer
wasmtime store, no separate WASI context, no separate fuel/epoch
deadline.

CLAUDE.md's "sandboxed by default" invariant is satisfied for the
*outer* render (which runs inside `akua-render-worker.cwasm` hosted by
wasmtime); a malicious *inner* Package, however, would inherit the
outer's caps and could exhaust the outer's wall-clock + memory budget
recursively. The original spike-1 plan recommended option (a): each
nested render runs in its **own wasmtime store**, sharing the
process-wide `Engine` but with an independent `WasiCtx`,
`StoreLimits`, and epoch deadline derived from the parent's remaining
budget.

## Findings

### Topology

- The outer render runs inside `RenderHost::shared().invoke_with_deps`
  (`crates/akua-cli/src/render_worker.rs`) which builds a fresh
  `Store<HostState>`, instantiates the AOT-compiled worker, and pipes
  a JSON request through WASI stdin.
- KCL plugin calls bridge back to the host via
  `plugin_bridge_call` → `kcl_plugin::invoke_bridge`.
- `pkg_render.rs` runs `PackageK::load(target).render(inputs)` on the
  host with **no nested store**. The boundary leaks here.

### Feasibility of option (a)

Yes — wasmtime's "one Engine, many Stores" model directly supports it.
The host bridge already holds `Caller<'_, HostState>` for the outer
store; nothing prevents constructing a new `Store` from the same
shared `Engine` and running `_start` on it inside the callback. The
KCL fork's reentrancy fix (`d584c0bc`) is the gate that made any
re-entry possible and doesn't care whether the re-entry is in-process
or store-nested.

Constraints:
- Cost per nested call: ~5–10ms (Module::instantiate + KCL bootstrap).
- Each nested store needs derived `ResourceLimits` (memory remaining =
  parent's headroom minus its current high-water-mark; epoch_deadline
  = remaining ticks). Live arithmetic surface that has to stay correct
  as `ResourceLimits` evolves.
- Host stack consumed across the bridge per level — needs an explicit
  depth cap (e.g. 16) on top of the existing cycle detection.

### Implementation footprint

| file | role | est. |
|---|---|---|
| `pkg_render.rs` | replace direct `PackageK::render` with host-injected callback | ~40 |
| `kcl_plugin.rs` | extend `RenderScope` with `BudgetSnapshot` | ~30 |
| `render_worker.rs` | `RenderHost::invoke_nested` + `register_pkg_render_host` | ~80 |
| `lib.rs`/`main.rs` | install the host callback at startup | ~5 |
| tests | nested-budget exhaustion, depth cap, parent-survives-child-OOM | ~150 |

Total ~300–400 LoC, 4 files of substance. One PR.

## Decision: defer

Ship a budget-cap header instead. Reasoning:

- **No untrusted upstream registry in v0.x.** `pkg.render` paths today
  are workspace-local relative paths that already pass the
  `resolve_in_package` escape guard. The threat model that justifies
  (a) — a malicious *published* Package — has no delivery surface
  pre-alpha.
- **Cost of (a) is recurring.** 5–10ms × every nested call adds up
  under composition (`examples/11-install-as-package` already shows
  2-deep nesting; real platform repos will hit 4–6). 300+ LoC of
  budget arithmetic that has to stay correct as `ResourceLimits`
  evolves.
- **Cheaper alternative covers the actual v0.x gap.** Add
  `BudgetSnapshot` (mem-used watermark via `StoreLimits` poll,
  wall-clock via parent's epoch deadline minus elapsed) to
  `RenderScope`; check it at the top of the `pkg.render` handler
  before calling into `PackageK::render`. Enforced at the Rust level,
  no fresh store. ~80 LoC, two files. Catches the "exhaust the
  parent's budget by recursive composition" failure mode — the only
  credible v0.x attack vector. Doesn't catch malicious WASI syscall
  patterns, but those have no delivery vector either.
- **Door stays open.** (a) is additive on top of the budget header —
  the header is what gets checked first whether or not we later move
  to nested stores. Doing the header now buys the safety story for
  v0.1 launch and lets (a) ship in the v0.2 cycle when a registry
  actually exists.

## Follow-up

- **Now:** budget-cap header for `pkg.render` (~80 LoC, separate
  task).
- **Post-registry:** revisit option (a) when published Packages
  become a real delivery vector. The work plan in §3 above is the
  starting point.
