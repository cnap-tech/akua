# Roadmap

Akua is being extracted from CNAP's internal chart generation service. The phased plan, from [CEP-0008](https://github.com/cnap-tech/cnap/blob/main/internal/cep/20260417-chart-transformation-platform.md):

| Phase | Status | Scope |
|-------|--------|-------|
| **v1** — Declarative install fields | ✅ Shipped (in CNAP, pre-extraction) | JSON Schema + `x-user-input` + `x-input` for templates/slugify/uniqueness |
| **v2** — Naming + template generalization | ⏳ Near-term | Multi-variable templates, `{{fields.*}}` / `{{install.*}}` scopes, vocabulary renames |
| **v3** — Single-runtime TS escape hatch | ⏳ Next cycle | `resolve.ts` via V8 isolate; `akua preview` CLI |
| **v4** — OSS extraction, Rust core, multi-runtime, OCI | 🚧 **This repo is home for v4 work** | Rust `akua-core`, Extism plugin host, pluggable source fetcher, OCI push, browser execution |
| **v5** — Package Studio IDE | 🔮 Multi-quarter | Full in-browser IDE, live reload, test runner, manifest diff, hot reload |
| **v6** — Upstream | 🔮 Ongoing | Propose values-transform HIP to Helm 4, contribute to Extism JS SDK |

## What v4 needs

Top priorities (tracked in the [v4 milestone](https://github.com/cnap-tech/akua/milestones) once populated):

1. **Source fetcher trait** with local Git, HTTP Helm repo, OCI registry, and browser-proxy implementations
2. **Umbrella chart generation** — port the Go/TS logic from CNAP's internal service to Rust; alias dependencies, merge values
3. **Extism plugin host** — WASM execution for transform logic; align with Helm 4's plugin ABI
4. **Schema merge + x-user-input extraction** — port from CNAP TS reference implementation
5. **OCI push** — via `oras` or Rust-native OCI crate
6. **CLI entry points** — `akua init`, `akua preview`, `akua test`, `akua build`, `akua publish`
7. **NAPI bindings** — for `@akua/core` (Node.js consumption)
8. **WASM bindings** — for browser consumption via `wasm-bindgen`

## What won't be in v4

- Full Package Studio IDE (that's v5)
- Python / KCL / CUE runtimes beyond WASM (follow-on)
- Upstream HIP proposal (v6)
- Marketplace integration (CNAP-proprietary, not in scope for Akua)
