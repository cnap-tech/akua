# Roadmap

> **Status:** v0.3 shipped the pre-pivot design (`package.yaml` + JSON-Schema authoring, `x-user-input` / `x-input` vocabulary, Helm-centric output). The project is pivoting to the interface-spec target: KCL-authored Packages, Rego-authored Policies, 30 verbs, `@akua/sdk` parity, embedded multi-engine pipeline, browser playground.
>
> This doc describes the **forward plan**. The historical v0.3 phases are archived below for continuity.

The strategic spine lives in the masterplan (private, internal). This doc is the operational "when" that matches the OSS-visible surface.

---

## Current state — v0.3 (pre-pivot)

What shipped:
- Rust core (`crates/akua-core`, `crates/akua-cli`) with `akua render / build / publish / inspect / attest`.
- Embedded Helm v4 engine via wasmtime (`crates/helm-engine-wasm`).
- Native KCL engine via `kcl-lang` Rust crate.
- Native OCI + HTTP chart-dep fetcher (no `helm dependency update` shell-out).
- `@akua/sdk` v0.3 on JSR (browser + Node entry points, SSRF guard, tar-bomb rejection).
- Security hardening: tar-symlink rejection, SSRF guard, cred redaction, CEL caller-timeout bound.

What this lets us do today: author a multi-source Helm package with JSON-Schema inputs, CEL expressions, publish a signed OCI artifact, render it back to manifests. Does not match the interface-spec target.

---

## Forward plan — toward the interface-spec target

The target is defined by the specs in `docs/`: [`cli.md`](./cli.md), [`cli-contract.md`](./cli-contract.md), [`package-format.md`](./package-format.md), [`policy-format.md`](./policy-format.md), [`lockfile-format.md`](./lockfile-format.md), [`embedded-engines.md`](./embedded-engines.md), [`sdk.md`](./sdk.md), [`agent-usage.md`](./agent-usage.md).

Implementation plan details (agent-driven rewrite, milestone criteria, task decomposition) live in [`impl-plan.md`](./impl-plan.md).

### Phase A — Foundation (weeks 0–6)

Goal: the interface-spec's load-bearing contracts exist as working code, even if surface is minimal.

- CLI skeleton honoring [`cli-contract.md`](./cli-contract.md): `--json`, `--plan`, typed exit codes, idempotency keys, `--timeout`, agent auto-detection, structured errors.
- `akua.toml` + `akua.lock` parser and resolver (go-mod shape; see [`lockfile-format.md`](./lockfile-format.md)).
- `Package.k` loader: parse KCL Package with the embedded `kclvm-rs` interpreter; extract schema, resolve imports.
- `akua check` (fast syntax + type check, no execution), `akua fmt`, `akua lint` (embedded Regal + kcl lint).
- `akua render` on a single-engine Package (KCL-only to start).
- `akua publish` + `akua verify` (cosign + SLSA v1); reuse v0.3 OCI client.
- `akua whoami` + agent context auto-detection (`CLAUDECODE` / `CURSOR_CLI` / `GEMINI_CLI` / `AGENT`).
- `@akua/sdk` typed wrapper with capability parity to the above verbs.

Exit gate: `akua render`, `akua publish`, `akua verify` round-trip on the [`examples/01-hello-webapp`](./examples/01-hello-webapp/) sample. `@akua/sdk.render()` produces byte-identical output to the CLI.

### Phase B — Multi-engine pipeline (weeks 6–12)

Goal: any source engine callable from a KCL Package produces deterministic bytes.

- Embedded Helm v4 (reuse `crates/helm-engine-wasm`; polish for `helm.template()` as KCL callable).
- Embedded Kustomize (via wasip1 Go→wasm).
- Embedded kro offline instantiator (`rgd.instantiate(...)`).
- Embedded CEL (already native; wire as KCL callable).
- Embedded Kyverno-to-Rego converter for policy pipeline.
- `akua render` handles mixed-engine Packages; input mapping validation against source schemas (`values.schema.json`, RGD `spec.schema`).
- `akua diff` (structural, cross-version).
- `akua inspect` full-tree output with provenance.

Exit gate: [`examples/02-webapp-postgres`](./examples/02-webapp-postgres/) (CNPG + webapp) renders end-to-end with byte-identical output across three calls of the same verb.

### Phase C — Policy engine (weeks 12–18)

Goal: Rego as host language for Policies with compile-resolved imports; embedded OPA.

- Embedded OPA evaluator.
- `akua.toml` compile-resolved imports: `import data.akua.policies.tier.production` pulls OCI artifact, verifies signature, mounts as Rego data.
- `akua policy check` verdict path (`allow` / `deny` / `needs-approval`).
- `akua test` for Rego (`*_test.rego`) + KCL (`test_*.k`).
- Policy tiers shipped as signed OCI artifacts: `tier/dev`, `tier/startup`, `tier/production`, `tier/audit-ready`.
- Workspace policy composition convention: local `.rego` files under `./policies/` importing tiers as compile-resolved data via `akua.toml`. No akua-owned PolicySet kind.

Exit gate: policy tier published + consumed round-trip. Deny verdict on an over-quota App is line-precise.

### Phase D — Deploy + dev loop (weeks 18–26)

Goal: the signature experience.

- `akua deploy` with reconciler drivers (`argocd`, `flux`, `kro`, `helm`, `kubectl`, custom). No non-K8s targets (see [`docs/architecture.md#what-akua-is-not`](./architecture.md)).
- `akua dev` content-addressable build graph, `localhost:5173` browser UI, sub-500ms edit-to-applied loop against a local K8s target (kind/k3d/minikube).
- `akua repl` (Rego + KCL).
- `akua trace`, `akua cov` for policy evaluation inspection.
- `akua query` against cluster-native Loki / Prom (no federation).

Exit gate: solo-developer journey from [`masterplan §19.1`](../../cortex/workspaces/robin/akua-masterplan.md) runs end-to-end on a fresh laptop in under 5 minutes.

### Phase E — Browser playground + Studio primitives (weeks 26–36)

Goal: the in-browser authoring surface.

- `akua.dev` playground: upload / paste a Package, live render, live diff, live lint.
- `@akua/ui` component primitives (PackageEditor, FormPreview, ManifestViewer, TestRunner) ship with working demos.
- Review surface template (open source UI component library; the hosted version lives in akua Cloud).

Exit gate: a public visitor can author a working Package in the browser, see rendered output, run policy check, view the structural diff against a prior version.

### Phase F — Agent-native refinements (continuous)

Not a discrete phase; every prior phase honors the agent contract. Specific ongoing work:

- Skills library ([`skills/`](./skills/)) grows to cover every common agent workflow.
- `next_actions[]` in every error structure, kept actionable.
- Terminal output budgets honored (<200 lines of scrollback in typical verb runs).

---

## Non-goals (in addition to [`design-notes.md §1`](./design-notes.md#what-akua-is-not))

- ❌ Non-Kubernetes deploy targets (Fly / Workers / Lambda / systemd).
- ❌ Cluster-side controllers for akua kinds.
- ❌ Runtime rendering in-cluster.
- ❌ Curated central package catalog.
- ❌ A PaaS — akua is a toolkit, not a hosted runtime.

---

## Out of scope for v1

- Cross-package composition (package-of-packages beyond umbrella deps).
- Python / CUE engines beyond KCL + existing embedded set.
- Upstream Helm 4 HIP contribution for template-function plugins (tracked at [helm#31498](https://github.com/helm/helm/issues/31498); post-v1).
- Gen-4 self-contained WASM renderer bundles (vision.md framing; strategic, not immediate).

---

## v0.3 phase history (pre-pivot, archived)

| Phase | Status | Scope |
|---|---|---|
| 0 — Pure algorithms | ✅ | djb2 hash, source parsing, value merging, schema extraction, transforms |
| 1a — Umbrella assembly | ✅ | Multi-source → umbrella Chart.yaml with aliases |
| 1b — Helm render | ✅ | Shell to `helm template`; `akua render` end-to-end |
| 1c — WASM bindings | ✅ | wasm-pack output; browser + Node consumable |
| 2a — Engine trait | ✅ | `Engine` trait + `PreparedSource` variants |
| 2b — CEL expressions | ✅ | `x-input.cel` via `cel-interpreter` |
| 2c — Provenance | ✅ | `.akua/metadata.yaml` on build |
| 3a — KCL engine | ✅ | Shelled to `kcl run`; later rewritten as native Rust (Phase 5) |
| 3b — helmfile engine | ✅ | Shells to `helmfile template`; gated off by default in later hardening |
| 4a — OCI publish | ✅ | Pure-Rust `oci-client`; Helm-compat media types |
| 4b — SLSA attestation | ✅ | SLSA v1 predicate via cosign |
| 5 — KCL as native Rust | ✅ | `kcl-lang` crate integration |
| 7a — Helm-engine WASM | ✅ | Go→wasip1 via wasmtime; `--engine=embedded` default |
| 7b — Native dep fetching | ✅ | oci-client + reqwest; zero `helm` CLI dep |
| 7c — Library hardening | ✅ | CWD-independent source paths |
| 7d — @akua/sdk on JSR | ✅ | 0.1 / 0.2 / 0.3 published |
| 7e — Security hardening | ✅ | SSRF, tar-symlink rejection, cred redaction |

v0.3 is the commit baseline from which the forward plan diverges. Most of the v0.3 code is carry-forward (OCI client, fetch, security guards, Helm engine embedding); some is superseded (`package.yaml` authoring, `x-user-input` vocabulary, `akua build`).
