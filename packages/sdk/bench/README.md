# `@akua-dev/sdk` benchmarks

Microbenchmarks that pin the SDK's cold + warm render cost so a future regression trips loud.

## Run

```sh
task sdk:bench                 # report only
task sdk:bench -- --check      # enforce the budget below; exit 1 on regression
```

## Current budget

Numbers below were the v0.6.x baseline on a developer macbook (M2 Pro, Bun 1.3, macOS 26). CI smoke validates the same numbers fit; the budget is the failure threshold, not the expected value.

| metric | budget | typical observed |
|---|---|---|
| cold (first call) | ≤ 500 ms | 30-40 ms |
| warm p50 | ≤ 25 ms | 0.4-0.6 ms |
| warm p95 | — (track only) | 2-3 ms |
| warm p99 | — (track only) | 4-6 ms |

The budget is intentionally loose — a 20-50× regression is the kind of thing this guard exists for, not a 10% drift.

## When to update the budget

- **Tighten** when the hot path stabilizes for a release cycle: confirm the new floor across runtimes (Bun, Node 22, Deno), then halve the budget so future regressions fail sooner.
- **Loosen** when adding a real feature on the hot path that's worth its cost (e.g., schema validation by default). Document the tradeoff in the commit message and the row above.

## What it doesn't cover

- Cross-runtime parity: this script runs on Bun by default; Node + Deno smoke runs are at `docs/sdk-runtime-compat.md`. CI doesn't fan out yet — tracked at #464.
- Warm-render perf with engines: `helm.template` / `kustomize.build` cost is dominated by the engine wasm (Go runtime startup, helm template engine), not the SDK overhead. A separate engine-call bench is a follow-up if those start regressing.
- Memory: not tracked. Worth adding if heap behavior becomes a concern.
