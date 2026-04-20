# Akua Design Notes

> **Scope.** This doc captures *why* Akua is shaped the way it is — the
> positioning, the invariants, the trade-offs that keep being re-discussed.
> Read this before proposing architectural changes.
>
> **Status:** Living document. Updated 2026-04-17. Owners: package team.
>
> **For "how it actually flows"** — author → install → ArgoCD → cluster,
> with both per-customer OCI and shared OCI models worked through —
> see [`use-cases.md`](./use-cases.md).
>
> **For the canonical spec of Akua's JSON Schema extensions** — see
> [`package-format.md`](./package-format.md) (UI hints via docstrings +
> `@ui` decorators).

This doc is the condensed operational version of Akua's design
reasoning, traveling with the OSS codebase.

---

## 1. Positioning

### What Akua is

- A **build tool** that turns a *package description* (sources + schema +
  transforms + optional engine-specific logic) into an **OCI-addressable
  Helm chart artifact**.
- The **contract + artifact + sandbox** wrapper around whatever templating
  engine the package author prefers. "Meta-packager."
- A **library** (Rust core + WASM bindings + CLI) so the same code runs in:
  - package authoring IDE (Package Studio),
  - customer install UI (in-browser live-preview),
  - server-side build workers (invoking `@akua/sdk`),
  - CI, local CLI, and AI coding agents (agents use the CLI via shell —
    no separate MCP server layer; the CLI *is* the agent interface).

### What Akua is *not*

- **Not a deploy tool.** ArgoCD / Flux own the cluster side. Akua hands
  them an OCI digest and steps away.
- **Not a helmfile / helm competitor.** Akua *composes* those tools —
  helmfile can be an engine plugin inside Akua.
- **Not a values language.** No DSL to learn to fill inputs; customers
  see a form driven by JSON Schema.

### The meta-packager thesis

Every package is:

```
authoring     →  build-time                            →  deploy-time
─────────────────────────────────────────────────────────────────────
user writes      Akua: engine plugin(s) emit                 ArgoCD /
package.yaml     chart fragments → umbrella chart →          Flux /
(+ schema,       OCI artifact (sealed, content-addressed)    helm
 transforms,                                                 install
 engine source)
```

The engine plugin (helm / helmfile / kustomize / KCL / jsonnet / native WASM)
runs at **authoring time only**. Its job is to *produce a Helm chart*. The
install-time runtime is always Helm.

---

## 2. Output invariant

### The chart is always a deployable Helm chart

No exceptions, no custom runtimes, no sidecars.

- ArgoCD / Flux support Helm charts natively (`oci://`).
- Customers can `helm install` the artifact directly.
- Every engine plugin compiles down to a chart dir — there is no "Akua
  deploy runtime."

### Three alternative paths we considered and rejected

| Alternative | Why rejected |
|---|---|
| Raw YAML directory via ArgoCD | Loses Helm hooks, release tracking, rollback semantics |
| ArgoCD Config Management Plugin (sidecar renderer) | Not portable (Flux has no CMP), needs cluster-admin install, moves eval to deploy-time → breaks content-addressing |
| Custom runtime / Akua agent in cluster | Adds operational burden, breaks "just a Helm chart" promise, doubles the deploy surface |

### What *is* in the chart

| Layer | Location | Always? | Strippable? |
|---|---|---|---|
| Helm standard | `Chart.yaml` (name, version, deps, annotations) | Yes | No |
| Install UI contract | `values.schema.json` (JSON Schema + `x-user-input` + CEL) | Yes | Yes, but then the chart is un-reconfigurable |
| Akua provenance | `.akua/metadata.yaml` | Default on | Yes (`akua build --strip-metadata` or `package.yaml: strip: true`) |
| SLSA attestation | **Separate** OCI artifact, adjacent to chart digest | Default on | Served as a peer via cosign convention; stripping chart doesn't break it |

### Content-addressable OCI

Same inputs → same OCI digest. Akua's transforms are deterministic by
design (no `now`, `rand`, env reads). Customers can verify "I'm running
the exact chart that was approved" via digest.

---

## 3. Lifecycle model — build-once, deploy-many

### Three distinct time axes

1. **Authoring time** — package dev writes + publishes a chart. Engines run here.
2. **Install time** — customer fills inputs. CEL transforms run. Resolved
   values computed. (Per customer. Cheap — no Helm build, no OCI push
   unless we opt in.)
3. **Deploy time** — ArgoCD/Helm renders chart + values to manifests.
   Always Helm. Always late-binding for helm-engine components.

### Two artifacts, separated by cost and lifecycle

- **The chart** — built once per package version by the dev.
  - Single OCI digest, shared across all customers.
  - Cached + CDN-fronted. Effectively free fan-out.
- **The resolved values** — per install, cheap to compute.
  - Customer fills form → CEL evaluates → small `values.yaml` (usually KB).
  - Either ArgoCD Application `values:` block, or tiny adjacent OCI blob.

### Update propagation

- Dev publishes chart v2 → one new OCI digest.
- Existing installs' Applications bump via Renovate.
- Each install re-evaluates CEL against v2's schema (fast, local).
- If v2 only adds optional fields → zero customer action.
- If v2 adds required fields → customer prompted once.

### When per-install chart builds *are* appropriate

Rare. Triggered by:

- Packages whose template logic varies per tenant (not just values).
- Engines that can't late-bind (some helmfile compositions, some kustomize
  patches). In that case Akua pre-renders at build time and ships a
  sealed chart with static `templates/` manifests.

This is the escape hatch, not the default.

---

## 4. Engine plugin contract

### Trait (conceptual)

```rust
pub trait EnginePlugin {
    fn build(&self, source: &Source, values: &Value) -> Result<HelmChartDir>;
    fn schema(&self, source: &Source) -> Result<Option<JsonSchema>>;
}
```

### Contract rules

1. Input: a source (path, URL, inline config) + resolved values.
2. Output: a directory Akua can pack as a Helm chart (`Chart.yaml`,
   `templates/`, `values.yaml`, optional schema fragment).
3. Deterministic: same inputs → same output bytes.
4. Sandboxed: no filesystem access outside the source dir, no network at
   build time (all fetching is Akua's job).
5. Runs at **authoring time only**. Never at install time.

### Shipped engines (roadmap)

- **helm** (native, v0.1) — trivial pass-through; source *is* a chart.
  Default late-binding.
- **helmfile-wasm** — wraps helmfile (Go→WASM via TinyGo or extism). Early
  binding (pre-renders to static templates) in most cases.
- **kustomize-wasm** — same shape as helmfile.
- **kcl-wasm** — KCL as an alt to Helm templating. Emits Helm chart.
- **wasm-plugin** — generic Extism plugin (Helm 4 HIP-0026 convention) for
  user-authored transforms. Useful for custom resolvers.

### Why helmfile as a plugin, not core

- Helmfile is a deploy-time tool by design — its model assumes operator
  authorship. Inside Akua, its role shifts: it becomes one way to *author*
  a package, which Akua then packages as a chart.
- Existing helmfile projects can migrate without rewriting — helmfile-wasm
  plugin accepts an existing `helmfile.yaml`.
- Keeps the core dependency-free; users who don't want helmfile never pay
  for it.

---

## 5. Multi-engine umbrella

### Mix engines per component

```yaml
# package.yaml
sources:
  - id: redis
    engine: helm                  # upstream Helm chart, pass-through
    chart: { repoUrl: …, chart: redis, targetRevision: 20.x }
  - id: app
    engine: kcl                   # KCL → rendered → wrapped as chart
    path: ./app-kcl/
  - id: ingress
    engine: kustomize             # kustomize → rendered → wrapped
    path: ./ingress-kustomize/
  - id: backup
    engine: helmfile              # helmfile → synthesized chart
    path: ./backup-helmfile.yaml
```

### How the merge works

1. For each source, Akua invokes its engine plugin → each returns a
   Helm-chart-shaped directory.
2. Akua's umbrella assembler (in `akua-core::umbrella`) aliases each as
   an umbrella dep, merges schema fragments, merges values under aliases.
3. One OCI artifact out. Vanilla Helm chart. `helm install` works.

### Cross-component wiring

- Does **not** cross plugin boundaries. Each plugin sees only its own
  source.
- Cross-component refs go through **Akua's values layer** (the merged,
  aliased values object). CEL expressions in `x-input` can reference
  other components' resolved values by alias.

### Per-component binding

- `helm` components keep templates (late-bind at deploy).
- `kcl`/`kustomize`/`helmfile` components ship pre-rendered static
  templates (early-bind at build).
- Both coexist in one umbrella. One install, mixed binding.

---

## 6. Customer inputs & expressions

### `x-user-input` — the form contract

```json
{
  "properties": {
    "customer": {
      "properties": {
        "name": {
          "type": "string",
          "title": "Customer name",
          "x-user-input": { "order": 10 }
        }
      }
    }
  }
}
```

Marks a field as customer-configurable. Drives:
- The install-UI form (rjsf or equivalent).
- Required-field validation.
- Order in the form.

### `x-input` — transforms

Today (v0.1) has a toy `{{value}}` template + `slugify` flag. This is
**insufficient** and slated for replacement.

### Replacing `{{value}}` with CEL

**Decision:** adopt CEL (Common Expression Language, Google) as the
inline expression language for `x-input`.

Why CEL:
- Sandboxed by spec (no exec, no env, no time, no network). Deterministic.
- WASM-compatible via `cel-rust` (~100KB runtime).
- **Kubernetes natively uses CEL** (ValidatingAdmissionPolicy, CRD
  validation). Operators already know it.
- Tiny grammar; small learning curve.
- Same expression surface can back `required` predicates, cross-field
  conditionals, and value transforms.

Example:
```json
"x-input": {
  "cel": "values.customer.name.lowerAscii() + '.' + values.environment + '.apps.example.com'"
}
```

Migration: keep `{{value}}` as sugar → auto-rewrite to `value` in CEL.
Deprecate the sugar once a few packages have migrated.

### CEL vs KCL (frequently conflated)

- **CEL** = expression language. One line → one value. Used for `x-input`.
- **KCL** = configuration language. Full typed program. Used as an *engine
  plugin* (compiles to a Helm chart), not for `x-input`.

They coexist. Different layers.

### `x-secret` — secret-field routing

Fields flagged `x-secret: true` should **never** land in `values.yaml`.
Routed instead to:
- Sealed Secrets,
- External Secrets Operator (ESO),
- Infisical,
- or an abstract `SecretStore` reference in the chart.

### `uniqueIn` — cross-install registry

For fields like public hostnames that must be unique across a tenant
pool, Akua queries a central registry at build time:

```json
"x-input": {
  "uniqueIn": "tenant.hostnames",
  "cel": "values.subdomain.lowerAscii()"
}
```

Registry lives in the hosting platform's infrastructure (not Akua core).

---

## 7. Browser live-preview (WASM bindings)

### Same core, four consumers

`akua-core` (Rust) compiles via `wasm-pack` to a browser-consumable
package. The same functions run in:

1. Native CLI (`cargo run -p akua-cli`) — also the surface AI agents
   call via their shells; no separate MCP server.
2. Browser (Package Studio IDE, customer install UI)
3. Node.js (build workers, tests)
4. CI

No TS reimplementation. No drift.

### UX win: zero-network live preview

Customer types `acme` into the `customer.name` field → WASM call evaluates
CEL → form shows the resolved `acme.staging.apps.example.com` in real
time. No server round-trip. Feels instant.

### Current WASM surface

- `extractUserInputFields(schema)`
- `applyInputTransforms(fields, inputs)` — will be upgraded to CEL
- `validateValuesSchema(schema)`
- `mergeHelmSourceValues(sources)`
- `buildUmbrellaChart(name, version, sources)`

Built via `task wasm:build` / `task wasm:smoke`.

---

## 8. Provenance & metadata

### The layers

- **Always:** `Chart.yaml` (standard Helm), `values.schema.json` (contract).
- **Default on, strippable:** `.akua/metadata.yaml`.
- **Default on, separate OCI artifact:** SLSA attestation + cosign sig.

### `.akua/metadata.yaml` example

```yaml
akua:
  version: 0.3.0
  buildTime: 2026-04-17T10:42:13Z
  sourceHash: sha256:…     # hash of package.yaml + inputs (not inputs themselves)
sources:
  - id: redis
    engine: helm
    origin: https://charts.bitnami.com/bitnami/redis
    version: 20.1.3
    digest: sha256:…
transforms:
  - field: httpRoute.hostname
    expression: "value.lowerAscii() + '.apps.example.com'"
    applied: true
```

### Why default on

- **Debuggability** — "why does this chart render X?" → trace lineage.
- **Supply-chain** — "which upstream charts am I actually shipping?"
- **Fork / re-author** — `akua inspect chart.tgz` reconstructs the
  package.yaml from metadata.
- **Akua tooling** (`inspect`, `diff`, `reproduce`) keys off it.

### Why strippable

- Source URLs can leak internal infra topology.
- Commercial distributions may want clean artifacts.
- Compliance teams sometimes want "just Kubernetes YAML, nothing else."

### Why SLSA lives outside the chart

- Sigstore convention: attestations are **peers** of the artifact, not
  children.
- Stripping metadata doesn't break signed provenance.
- cosign-verifiable without unpacking the chart.

---

## 9. Phase status

Phases 0–4 landed. Current state + upcoming work lives in
[`roadmap.md`](./roadmap.md) to keep this doc focused on *why*.

### Non-goals (explicit)

- ❌ Replacing Helm as a deploy runtime. Helm stays.
- ❌ Replacing ArgoCD / Flux. They consume Akua output.
- ❌ Custom Kubernetes controllers for Akua packages. The chart is the
  deliverable.
- ❌ A DSL for end users. JSON Schema + CEL is the surface.
- ❌ Runtime rendering in cluster. All renders happen before OCI push.

---

## 10. Engine determinism reality check

The original vision was "all engines run as Extism WASM plugins →
sandboxed, deterministic, browser-portable." Reality after deep
research (Q2 2026):

### Where WASM works today

| Engine | WASM story | Akua status |
|---|---|---|
| **Akua core** (schema extraction, CEL, umbrella assembly, metadata, attest) | Native Rust + wasm-pack. Deterministic. | ✅ **Actually WASM.** Used from the browser. |
| **KCL** | Evaluator is Rust-native. Official SDK crate `kcl-lang` at [kcl-lang/lib](https://github.com/kcl-lang/lib) (git dep — not on crates.io yet). Compiles in-process; no subprocess, no fetch. For browser: official `kcl.wasm` + [`@kcl-lang/wasm-lib`](https://www.npmjs.com/package/@kcl-lang/wasm-lib) npm package. | ✅ Native Rust in CLI (Phase 5); browser uses JS-side `@kcl-lang/wasm-lib`. |
| **Helm template** | Compiled `helm.sh/helm/v4/pkg/engine` to wasip1 reactor module via `-buildmode=c-shared`, embedded via `include_bytes!`, hosted via wasmtime. Full Helm semantics. | ✅ `akua render --engine helm-wasm` is default; no `helm` CLI needed. ~75 MB wasm (size-optimisation to ~15 MB via fork tracked as backlog). |
| **Helmfile** | Pure Go with heavy deps (helm v3+v4, client-go, AWS SDK). TinyGo blocked (no reflection, no `os/exec`). Mainline Go `wasip1` links but `helm` shell-outs fail at runtime. Helmfile's value IS orchestration of helm — embedding doesn't help. | 🟡 CLI shell-out kept. Honest caveat doc'd in `examples/helmfile-package/README.md`. |
| **OCI publish** | `oci-client` (Rust, from oras-project). Pure-Rust push of Helm charts with matching media types + OCI annotations mirrored bit-for-bit from `helm/helm/pkg/registry/client.go` Push. | ✅ `akua publish` ships native; no `helm push` / oras CLI needed. |
| **Extism** | Rust SDK stable (1.21.0). Deny-all sandbox. HIP-0026 shipped in Helm 4.0 **but covers only Download/Postrender/CLI plugins** — template-function plugins deferred ([helm#31498](https://github.com/helm/helm/issues/31498)). Determinism is plugin-author discipline, not framework-enforced. | ⏭ Right ABI for future custom plugins; not wired yet. |

### Three takeaways

1. **Akua's own thesis is delivered for its own code.** The schema
   extraction, CEL evaluation, umbrella assembly, metadata emission,
   and SLSA predicate generation are deterministic pure functions that
   run identically in native Rust, Node, and browser WASM.
2. **Engine-provided determinism is honest-best-effort.** A KCL source
   is deterministic by KCL's language design. A Helm chart inherits
   Helm's Sprig non-determinism (`now`, `randAlpha`, `env`). A
   helmfile inherits helmfile's looser defaults unless
   `HELMFILE_DISABLE_INSECURE_FEATURES=1`.
3. **The `.akua/metadata.yaml` block records the determinism level
   per source** so downstream verifiers know what they can trust.

### What we explicitly ruled out

- **Compiling upstream Helm to WASM ourselves.** Every attempt fails
  on `client-go` + OCI pulls + `os/exec`. Go→WASM requires either
  stripping deps (not viable) or providing shims for hundreds of
  syscalls (expensive, fragile).
- **Pure Rust Helm template renderer.** `gtmpl-rust` + clean-room
  Sprig ports exist but cover ~60–80% of real charts. Perpetual
  semantic drift risk with every Helm release. Not worth it.
- **Shipmight/helm-playground as a Helm WASM substrate.** It markets
  as "Helm in the browser" but doesn't embed Helm — imports only
  Sprig + text/template, stubs `required`/`fail`/`lookup`, has no
  subchart support. AGPL-3.0 licensing blocks vendoring anyway.

Findings captured here so we don't re-research. Revisit if:
- Go's WASM story improves enough to compile mainline Helm.
- A production-grade pure-Rust Helm renderer appears.
- Helm upstream adds a native WASM backend (unlikely pre-Helm 5).

---

## 11. Open questions

1. **`uniqueIn` semantics.** Registry protocol? Who holds state? Akua
   side has a trait; the hosting platform has the impl. Needs API spec.
2. **Per-install upgrade UX.** When v2 adds required fields, how do we
   prompt customers without nagging? Notification via the hosting
   platform, probably; Akua exposes a "needs attention" diff API.
3. **Engine plugin distribution.** OCI artifact? Crates.io? npm? Extism
   has a convention (OCI). Lean toward that.
4. **Chart signing.** cosign is consensus; but do we sign the chart
   itself too, or only the SLSA attestation? Probably both.
5. **Package-level secrets.** Can a package declare secrets it needs
   from the platform (e.g., "requires a Postgres connection")? Maybe a
   `requires:` field; orthogonal to user-input.
6. **Cross-package composition.** If package A depends on package B,
   how? Umbrella-of-umbrellas is straightforward; API for referencing
   another Akua package by OCI digest is not.

---

## Appendix: one-line glossary

- **Package** — the authored unit (sources + schema + engine config).
- **Chart** — the Akua output artifact (vanilla Helm chart).
- **Install** — one deployment of a chart for one customer.
- **Source** — one component within a package (a chart, KCL dir, etc.).
- **Engine** — the tool that turns a source into a chart fragment.
- **Umbrella** — the top-level Chart.yaml that aliases all sources.
- **Transform** — CEL-computed value derived from user inputs.
- **Alias** — `<chart-name>-<hash>`; distinguishes multiple sources sharing a chart.
- **x-user-input** — schema extension marking a field customer-configurable.
- **x-input** — schema extension holding the transform config (CEL, slugify, uniqueIn).
- **x-secret** — schema extension routing a field to a SecretStore.
