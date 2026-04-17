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
| **4a — OCI publish** | ✅ Landed | `akua publish` via `oci-client` (pure Rust, no `helm` CLI); Helm-compat media types + annotations; returns OCI digest |
| **4b — SLSA attestation** | ✅ Landed | `akua attest` emits SLSA v1 predicate for cosign + adjacent OCI push |
| **5 — KCL as native Rust** | ✅ Landed | `engine: kcl` now calls the native `kcl-lang` Rust crate (git dep). No subprocess, no fetch, 4 ms eval. Browser renders via `@kcl-lang/wasm-lib` + JS glue (akua-wasm doesn't compile a KCL engine). |
| **6 — Install UI reference** | 🔮 Near-term | React + rjsf + WASM bindings; demos the customer-facing flow end-to-end |
| **7 — MCP server** | 🔮 Near-term | `akua mcp` — tools for AI coding agents |
| **8 — Helm-engine WASM** | 🔮 Multi-quarter | Go→WASM wrapper around `helm/v3/pkg/engine`, hosted via wasmtime |
| **9 — Package Studio IDE** | 🔮 Multi-quarter | Full in-browser authoring IDE |
| **10 — Upstream** | 🔮 Ongoing | HIP proposals to Helm (template-function plugins), Extism contributions |

## Phase 5 — KCL as native Rust (landed)

Research into Rust/WASM embedding paths ([design-notes.md §11](./design-notes.md#11-engine-determinism-reality-check))
surfaced the right call: KCL's evaluator is already Rust. Linking it
directly is cleaner than embedding `kcl.wasm` via wasmtime.

**Shipped:**
- `engine: kcl` uses the `kcl-lang` crate (git dep, no crates.io publish yet).
- No subprocess, no kcl binary on `$PATH`, no wasm blob to fetch.
- Native perf (~4ms per eval, 12MB binary).
- First build ~3.5min; cached thereafter.

**Browser handling (composed, not unified):**
- akua-wasm does NOT include a KCL engine (heavy Rust deps can't
  compile to wasm32).
- Browser apps render KCL via upstream's `@kcl-lang/wasm-lib` npm
  package and pass the rendered YAML into akua-wasm's umbrella
  assembler via the Engine trait boundary.
- One spec, two implementations, identical behavior.

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
