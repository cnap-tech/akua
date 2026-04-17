# Roadmap

Akua is being extracted from CNAP's internal chart generation service. See
[`design-notes.md`](design-notes.md) for the *why*; this doc is the *when*.

## Phase status

| Phase | Status | Scope |
|-------|--------|-------|
| **0 — Pure algorithms** | ✅ Landed | djb2 hash, source parsing, value merging, schema extraction, transforms. 77 unit tests. |
| **1a — Umbrella assembly** | ✅ Landed | Multi-source → umbrella Chart.yaml with aliases. `akua tree` + `akua build`. |
| **1b — Helm render** | ✅ Landed | Shell to `helm template`. `akua render` end-to-end. |
| **1c — WASM bindings** | ✅ Landed | wasm-pack output; browser + Node consumable; shared core with CLI. |
| **2 — Meta-packager foundations** | 🚧 Active | Engine trait, CEL expressions, provenance metadata. |
| **3 — First alt-engine plugin** | ⏭ Next | KCL or helmfile (lives in separate repo). |
| **4 — Distribution surface** | ⏭ Next | OCI push (`oras`), SLSA/cosign, install UI template, MCP server. |
| **5 — Package Studio IDE** | 🔮 Multi-quarter | Full in-browser authoring IDE. |
| **6 — Upstream** | 🔮 Ongoing | HIP proposals to Helm, contributions to Extism. |

## Phase 2 — Meta-packager foundations (active)

Make Akua a true meta-packager rather than a helm-only tool.

- [ ] Extract `EnginePlugin` trait from the current `render` module. Helm
      becomes one impl of many.
- [ ] Add `engine:` to `package.yaml` (default `helm`). Reserved values
      for planned plugins.
- [ ] Stub `WasmPluginEngine` backed by Extism — proves the contract
      without shipping a full alternate engine yet.
- [ ] Replace `x-input.template` ("{{value}}") with `x-input.cel` via
      `cel-rust`. Keep template syntax as sugar during migration.
- [ ] Emit `.akua/metadata.yaml` on `akua build` (source lineage, engine
      mix, transform audit, Akua version).
- [ ] `akua inspect chart.tgz` — show the metadata block, verify
      provenance.

## Phase 3 — First alt-engine plugin

Prove the plugin contract with a real alternate engine. Candidates:

1. **KCL** — cleanest target (already emits YAML). Repo:
   `cnap-tech/akua-engine-kcl`.
2. **helmfile-wasm** — biggest migration story for existing helmfile
   users. Repo: `cnap-tech/akua-engine-helmfile`. Complicated — Go→WASM.
3. **kustomize-wasm** — similar shape to helmfile.

Pick based on actual user demand once Phase 2 lands.

## Phase 4 — Distribution surface

- [ ] `akua publish --to oci://…` via `oras`.
- [ ] SLSA attestation + cosign signature, adjacent OCI artifact.
- [ ] Reference install-UI template — React + rjsf + WASM bindings,
      demonstrates the customer-facing flow end-to-end.
- [ ] `akua mcp` — MCP server exposing tools to AI coding agents.

## Explicit non-goals

- ❌ Replace Helm as a deploy runtime.
- ❌ Replace ArgoCD / Flux.
- ❌ Ship a custom Kubernetes controller for Akua packages.
- ❌ Invent a new config DSL for end users (JSON Schema + CEL is enough).
- ❌ Runtime rendering in cluster.
- ❌ CNAP-proprietary features — marketplace, tenant isolation, secret
      stores — stay on the CNAP side. Akua stays stateless per install.

## Out of scope for v0.x (possibly later)

- Python / CUE engines — only after KCL / helmfile prove the plugin path.
- Cross-package composition (package-of-packages beyond umbrella deps).
- Upstream Helm 4 HIP contribution for values transforms — Phase 6.
