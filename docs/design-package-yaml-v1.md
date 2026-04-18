# `package.yaml` v1alpha1 — final design

> **Status:** implemented. Parser enforces the shape below; all examples
> are authored in v1alpha1; no v0 reader exists (hard cut — no published
> packages to migrate).
> **Confidence:** ~90% on shipped surface; see "Deferred" section for the
> 55–65% ideas we're *not* shipping in v1alpha1.
> **Companion:** [`design-package-yaml-research.md`](./design-package-yaml-research.md)
> captures the raw findings from helmfile, kustomize, Flux, Argo, Timoni, Porter.

## Final shape

```yaml
apiVersion: akua.dev/v1alpha1

name: hello-package
version: 0.1.0
description: Optional human-readable description.

schema: ./values.schema.json

sources:
  - name: app                                       # required, immutable-by-convention
    helm:                                           # engine discriminated by block presence
      repo: https://charts.bitnami.com/bitnami
      chart: nginx
      version: 18.1.0
    values:
      replicaCount: 1

  - name: custom
    kcl:
      entrypoint: ./app.k
      version: 0.1.0

  - name: stack
    helmfile:
      path: ./helmfile.yaml
      version: 0.1.0
```

That's the whole format.

## Decisions (all settled)

| Decision | Final | Rationale |
|---|---|---|
| Schema discriminator | `apiVersion: akua.dev/v1alpha1` | Chart.yaml precedent. Group scopes ownership. `v1alpha1` carries the "expect changes" social contract. |
| `kind:` / `metadata:` / `spec:` wrappers | **None.** Flat fields at top level. | Chart.yaml, Cargo.toml, package.json, Porter — every build-time manifest goes flat. CRD wrappers are for cluster resources. |
| Package identity | `name:`, `version:`, `description:` at top | Flat like Chart.yaml. |
| Customer-input schema | `schema: ./values.schema.json` (separate file) | Already conventional. Keeps YAML focused on composition. |
| Source identifier | `name:` (required, immutable-by-convention) | K8s `metadata.name` convention. Previously `id: Option<String>` — making it required simplifies alias computation and matches downstream expectations. |
| Engine discrimination | **Block presence.** No `engine:` field. | DRY — `engine: helm` + `helm: {...}` repeated the info. Argo and Cargo both discriminate by key presence. Strict parse-time validation catches typos. |
| Per-engine config | Typed block per engine (`helm:` / `kcl:` / `helmfile:`) | Fixes the prior shoehorning of `chart.repoUrl` = `file://./app.k` for KCL. |
| Version field name | `version:` (Helm/KCL/helmfile convention) | Not Argo's `targetRevision:`. |
| Values format | Typed YAML only. Deep-merge maps, replace arrays. | ArgoCD shipped `values` (string) → `valuesObject` (typed); the string form was the #1 complaint. Skip the mistake. |
| File structure | Single `package.yaml`. `package.d/` deferred. | Both helmfile and kustomize confirm single-file scales surprisingly far. Easy to add later; annoying to remove. |
| Graduation policy | `v1alpha1` → `v1`. Skip `beta` unless genuinely needed. | Kustomize-stuck-at-beta is a failure of graduation scheduling, not of the shape. |

## Immutability of `name`

Treated as immutable after a package's first publish. Technically mutable, but
changing it changes the identity of the source downstream:

- **Per-source alias** (`<chart-name>-<hash-of-name>`) changes — subchart
  references and umbrella dep entries shift.
- **Cross-source refs** (once added — see "Deferred") break.
- **Schema paths** reference sources by `name`; customer-facing forms break.

Same semantics as Chart.yaml's `name:` or a k8s resource's `metadata.name`.
Tooling warnings will surface this when a user tries to rename.

## Strict validation rules

Enforced at parse time, with actionable errors:

- **Exactly one engine block per source.** Zero → `source "app" declares no engine (expected one of: helm, kcl, helmfile)`. Two → `source "app" declares both helm and kcl; exactly one allowed`.
- **Unknown keys rejected, not silently dropped.** `source "app" has unknown field "hlem" (did you mean "helm"?)`.
- **Required fields enforced.** `source missing required field "name"`.
- **Name uniqueness within a package.** Two sources named `app` is an error.
- **API version recognised.** Unknown `apiVersion:` → `unknown apiVersion "akua.dev/v2"; this akua version supports: akua.dev/v1alpha1`.

## Deferred to v1alpha2+ (all additive)

Each of these was in the earlier proposal but at 55–65% confidence — not
enough to commit the surface area before real user friction tells us what
shape users actually want. All are *additive* top-level fields that land
cleanly in a later alpha without breaking v1alpha1 packages.

### `environments:` (hierarchical axis)

Workaround today: maintain separate package.yaml files per env, or pass
`--values prod.yaml` at build time. Add when the first user complains about
duplication.

### `components:` (orthogonal axis)

Workaround today: separate package.yaml files or inline `values:` overrides.
Add when the second user asks about optional feature flags (tls, monitoring,
hpa).

### Cross-source CEL refs `${sources.<name>.values.<field>}`

Neither Porter's dependency DAG nor Timoni's runtime-lookup got this right,
which is evidence the problem is *hard*, not that our specific design is
correct. Ship when a concrete use case validates the evaluation semantics
(see "Semantics when we add it" below).

**Semantics when we add it:**
- Evaluation is topologically ordered by ref dependency — error on cycles.
- Refs see the *fully-merged* values of the referenced source (user input →
  CEL → environment patches → component patches), not raw input.
- Missing ref → error (never silent empty string).

## Explicitly rejected (not deferred — rejected on design grounds)

- **Flux's `spec.chart.spec.*` double-nest.** Historical artifact.
- **Porter's mixin-as-binary contract.** Fragmented ecosystem.
- **Timoni's CUE-only schema.** CUE learning cliff. Akua uses JSON Schema.
- **Helmfile's Go-template-the-YAML.** Non-determinism is exactly what
  Akua's thesis rules out.
- **Multiple patch dialects.** One merge semantic: deep-merge maps, replace
  arrays.
- **Values as string-of-YAML.** Only typed YAML, ever.

## Migration

`akua migrate-v1` mechanically converts the current format:

| v0 field | → | v1alpha1 field |
|---|---|---|
| `source.id` | → | `source.name` (required) |
| `source.engine: helm` + `chart.repoUrl` + `chart.chart` + `chart.targetRevision` | → | `source.helm.{repo, chart, version}` |
| `source.engine: kcl` + `chart.repoUrl` (file://) | → | `source.kcl.{entrypoint, version}` |
| `source.engine: helmfile` + `chart.repoUrl` (file://) | → | `source.helmfile.{path, version}` |
| No top-level `apiVersion` | → | `apiVersion: akua.dev/v1alpha1` prepended |

One-time breaking change. No published packages exist yet → migration cost
is effectively zero. Ship with both readers for one release as a safety net;
remove v0 reader in the release after.

## What's *not* changing

- The `values.schema.json` schema file shape (JSON Schema + `x-user-input` +
  `x-input.cel`). Orthogonal to package.yaml.
- Engine plugin internals (trait, dispatch, render path). The rename from
  `id` → `name` touches the Rust `HelmSource` struct but doesn't change
  behavior.
- Output: akua still produces a Helm chart, pushes via OCI, attests via
  SLSA. Build tool contract unchanged.
- Helm v4 template engine embedding, native dep fetching, native OCI push.

## Implementation

Landed in a single cut (no v0 reader — zero published packages, so no
migration path to maintain):

- `akua-core::manifest::PackageManifest` carries `apiVersion`, parses
  with `#[serde(deny_unknown_fields)]`, validates via `validate_manifest`
  (unknown apiVersion, missing/duplicate/multi engine blocks).
- `akua-core::source::Source` replaces the prior `HelmSource` +
  `ChartRef`. Per-engine blocks: `HelmBlock { repo, chart?, version }`,
  `KclBlock { entrypoint, version }`, `HelmfileBlock { path, version }`.
- `engine::prepare` dispatches on `Source::kind()` instead of a string.
  No `DEFAULT_ENGINE` constant; no `engine::resolve(&str)`.
- `examples/{hello,kcl,helmfile}-package/package.yaml` and
  `packages/core-wasm/smoke-test.mjs` are authored in v1alpha1.
- SLSA provenance compatibility preserved: `SourceInfo` still serializes
  as `{ id, engine, origin, version, alias }` — the Rust field is
  renamed to `name` but `#[serde(rename = "id")]` keeps the output key.

Estimate: 2–3 focused days.
