# Roadmap

Akua is being extracted from CNAP's internal chart generation service. See
[`design-notes.md`](design-notes.md) for the *why*; this doc is the *when*.

## Phase status

| Phase | Status | Scope |
|-------|--------|-------|
| **0 — Pure algorithms** | ✅ Landed | djb2 hash, source parsing, value merging, schema extraction, transforms |
| **1a — Umbrella assembly** | ✅ Landed | Multi-source → umbrella Chart.yaml with aliases; `akua tree`, `akua build` |
| **1b — Helm render** | ✅ Landed | Shell to `helm template`; `akua render` end-to-end |
| **1c — WASM bindings** | ✅ Landed | wasm-pack output; browser + Node consumable; shared core with CLI |
| **2a — Engine trait** | ✅ Landed | `Engine` trait + `PreparedSource::{Dependency,Git,LocalChart}`; `engine:` in package.yaml |
| **2b — CEL expressions** | ✅ Landed | `x-input.cel` via `cel-interpreter`; `{{value}}` kept as sugar |
| **2c — Provenance** | ✅ Landed | `.akua/metadata.yaml` on build; `akua inspect` reads it back; `--strip-metadata` opt-out |
| **3a — KCL engine** | ✅ Landed | Shells to `kcl run`; writes static subchart; `examples/kcl-package` |
| **3b — helmfile engine** | ✅ Landed | Shells to `helmfile template`; static subchart; `examples/helmfile-package` |
| **4a — OCI publish** | ✅ Landed | `akua publish` via `helm push`; returns OCI digest |
| **4b — SLSA attestation** | ✅ Landed | `akua attest` emits SLSA v1 predicate for cosign + adjacent OCI push |
| **5 — KCL as embedded WASM** | 🚧 Next | Replace CLI shell-out with official `kcl.wasm` via wasmtime — real determinism + browser portability |
| **6 — Install UI reference** | 🔮 Near-term | React + rjsf + WASM bindings; demos the customer-facing flow end-to-end |
| **7 — MCP server** | 🔮 Near-term | `akua mcp` — tools for AI coding agents |
| **8 — Helm-engine WASM** | 🔮 Multi-quarter | Go→WASM wrapper around `helm/v3/pkg/engine`, hosted via wasmtime |
| **9 — Package Studio IDE** | 🔮 Multi-quarter | Full in-browser authoring IDE |
| **10 — Upstream** | 🔮 Ongoing | HIP proposals to Helm (template-function plugins), Extism contributions |

## Phase 5 — KCL as embedded WASM (next)

The research in [`design-notes.md §12`](./design-notes.md#12-engine-determinism-reality-check)
showed KCL is the only engine with a real path to in-process / WASM
execution today. Plan:

- [ ] Add a `wasmtime` dep behind a new `engine-kcl-wasm` feature.
- [ ] Pin `kcl.wasm` as a build artifact (SHA-locked).
- [ ] Rewrite `engine::kcl` to load the module, call `kcl_run`, capture
      output without a subprocess.
- [ ] Make WASM the default; `engine-kcl-cli` remains as a fallback for
      contributors who don't want the wasm runtime.
- [ ] Verify the same `kcl.wasm` drops into `akua-wasm` so the browser
      can render KCL packages live.

## Phase 6 — Install UI reference

- [ ] React + rjsf app reading a chart's `values.schema.json` from OCI.
- [ ] Browser calls `@akua/core-wasm` for live CEL preview.
- [ ] Submits resolved values → Akua builder produces per-install chart
      (or, for Model A, just writes values into ArgoCD Application).
- [ ] Published as `examples/install-ui/` — not a product, a template.

## Phase 7 — MCP server

- [ ] `akua mcp` implements the Model Context Protocol surface:
      `akua.preview`, `akua.lint`, `akua.build`, `akua.attest`,
      `akua.inspect`. Tools for AI coding agents to author packages.

## Phase 8 — Helm-engine WASM (multi-quarter)

The honest gap: `akua render` still shells to `helm`. The viable path
is a tiny Go wrapper around `helm.sh/helm/v3/pkg/engine.Render`
compiled to `wasip1`, hosted via wasmtime from Rust. Keeps full Helm
semantics (Sprig, named templates, subcharts, `.Files`,
`.Capabilities`) without the CLI.

Not prioritized until Phase 5 proves the wasmtime-embedded-engine
pattern with KCL.

## Explicit non-goals

- ❌ Replace Helm as a deploy runtime.
- ❌ Replace ArgoCD / Flux.
- ❌ Ship a custom Kubernetes controller for Akua packages.
- ❌ Invent a new config DSL for end users (JSON Schema + CEL is enough).
- ❌ Runtime rendering in cluster.
- ❌ CNAP-proprietary features — marketplace, tenant isolation, secret
      stores — stay on the CNAP side. Akua stays stateless per install.

## Out of scope for v0.x (possibly later)

- Python / CUE engines — only after KCL WASM validates the embedded-
  runtime pattern.
- Cross-package composition (package-of-packages beyond umbrella deps).
- Upstream Helm 4 HIP contribution for template-function plugins (HIP
  after HIP-0026 lands). Currently stub-tracked at [helm#31498](https://github.com/helm/helm/issues/31498).
