# Roadmap

akua originated as an internal chart generation service and is being
published as a standalone toolkit. See
[`design-notes.md`](design-notes.md) for the current-design *why*,
[`vision.md`](vision.md) for the long-range Gen 4 *ambition*; this doc is
the *when*.

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
| **7a — Helm-engine WASM** | ✅ Landed | Go→wasip1 wrapper around `helm/v4/pkg/engine` hosted via wasmtime. `akua render --engine helm-wasm` is the default. Kills `helm template` shell-out. |
| **7b — Native dep fetching** | ✅ Landed | `akua-core::fetch` pulls OCI + HTTP chart deps in-process (oci-client + reqwest). Replaces `helm dependency update`. Default render now has zero `helm` CLI dep. |
| **7c — Library hardening** | ✅ Landed | Dropped `std::env::set_current_dir` in `load_package`; source paths absolutised up-front so engines are CWD-independent. Safe for concurrent / multi-threaded library use. |
| **7d — @akua/sdk on JSR** | ✅ Landed | 0.1/0.2/0.3 published to JSR. Node entry + browser entry + `@akua/sdk/cache` Node-only. `pullChart` (OCI + HTTP Helm), `packChart` with metadata/schema, `dockerConfigAuth`, streaming, LRU cache, SSRF guard, tar bombs + symlink escapes blocked. See `SECURITY.md`. |
| **7e — Security hardening** | ✅ Landed | P0 tar symlink rejection, P0 helmfile gated behind opt-in feature, P1 SSRF guard (Rust + SDK), P1 source-path confinement, credential redaction in Debug + error messages, CEL timeout + source cap, strict OCI media type, LRU cache eviction. |
| **8 — Install UI reference** | 🔮 Near-term | React + rjsf + `@akua/sdk/browser`; demos the customer-facing flow end-to-end |
| **9 — Gen 4 bundle output** | 🔮 Post-v1 | `akua publish --bundle` emits multi-layer OCI: engine.wasm (shared digest) + sources + schema + attestation. Reference consumer `akua render-bundle <oci-ref>`. See [`vision.md`](vision.md). |
| **10 — Gen 4 ecosystem bridges** | 🔮 Post-v1 | `akua-cmp` sidecar for ArgoCD, Flux post-renderer plugin, `helm install --wasm` plugin. De-facto spec before CNCF standardisation. |
| **11 — Package Studio IDE** | 🔮 Multi-quarter | Full in-browser authoring IDE |
| **12 — Upstream** | 🔮 Ongoing | HIP proposals to Helm (template-function plugins), Extism contributions, CNCF TAG App Delivery spec pitch |

## Phase 5 — KCL as native Rust (landed)

Research into Rust/WASM embedding paths ([design-notes.md §10](./design-notes.md#10-engine-determinism-reality-check))
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

## Phase 7 — Single-binary rendering (landed)

The thesis: `akua render` has **zero external CLI dependencies** for
the default flow. Three pieces shipped:

- **7a** — [`crates/helm-engine-wasm`](../crates/helm-engine-wasm/README.md):
  Go wrapper around `helm.sh/helm/v4/pkg/engine.Render` compiled to
  wasip1 (reactor module via `-buildmode=c-shared`), embedded into the
  akua binary via `include_bytes!`, hosted via wasmtime. Full Helm
  semantics (Sprig, named templates, subcharts, `.Files`,
  `.Capabilities`) with no `helm template` shell-out.
- **7b** — `akua-core::fetch`: native OCI + HTTP Helm-repo chart
  fetcher (oci-client + reqwest). Replaces `helm dependency update`.
- **7c** — `load_package` no longer mutates the process CWD. Source
  paths absolutised up-front. Safe for concurrent library use
  (backend services, server embedding, etc.).

**Size optimisation (backlog):** the embedded `helm-engine.wasm` is
~75 MB because Go's linker can't prune types exposed through
`pkg/engine`'s public API (e.g., `rest.Config`). A forked build that
vendors just the template engine + strips the `k8s.io/client-go`
import would land at ~15 MB. Not worth the upstream-sync burden
until users complain about binary size. See
[`crates/helm-engine-wasm/README.md`](../crates/helm-engine-wasm/README.md)
"Option 2" for the detailed plan.

**Legacy:** `--engine helm-cli` remains for users who prefer shelling
to their installed Helm. Helmfile engine still shells to `helmfile`
(its whole point is orchestrating helm — embedding doesn't make sense).

## Explicit non-goals

- ❌ Replace Helm as a deploy runtime.
- ❌ Replace ArgoCD / Flux.
- ❌ Ship a custom Kubernetes controller for Akua packages.
- ❌ Invent a new config DSL for end users (JSON Schema + CEL is enough).
- ❌ Runtime rendering in cluster.
- ❌ Host-platform features — marketplace, tenant isolation, secret
      stores — stay with the hosting platform. Akua stays stateless
      per install.

## Out of scope for v0.x (possibly later)

- Python / CUE engines — only after KCL WASM validates the embedded-
  runtime pattern.
- Cross-package composition (package-of-packages beyond umbrella deps).
- Upstream Helm 4 HIP contribution for template-function plugins (HIP
  after HIP-0026 lands). Currently stub-tracked at [helm#31498](https://github.com/helm/helm/issues/31498).
