# `package.yaml` v1 — design for review

> **Status:** proposal, not implemented. User feedback wanted before the
> breaking change lands. No published packages exist yet → migration cost
> is ~zero **right now** and rises fast once external users adopt.

## Why change the current format

Current shape (shipped):

```yaml
name: hello-package
version: 0.1.0
sources:
  - id: app
    engine: helm
    chart:
      repoUrl: https://charts.bitnami.com/bitnami
      chart: nginx
      targetRevision: 18.1.0
    values:
      replicaCount: 1
```

**Problems** surfaced by researching helmfile, kustomize, Flux HelmRelease,
ArgoCD Application, Timoni, and Porter (summary in
[`design-package-yaml-research.md`](./design-package-yaml-research.md)):

1. **Shoehorned per-engine config.** KCL sources abuse `chart.repoUrl` to
   mean "path to `.k` file". helmfile sources do the same with
   `helmfile.yaml`. Bad abstraction — `chart:` is meaningful only for the
   `helm` engine. Every well-designed tool (Argo, Timoni, Porter) has
   per-type sub-blocks.
2. **ArgoCD terminology leak.** `targetRevision` is Argo-specific; means
   nothing for KCL / helmfile / any future engine.
3. **No environments / overlays.** Kustomize added its `components`
   concept after 4 years of "we wish we'd designed for two composition
   axes upfront." We can bake that in free right now.
4. **No cross-source value wiring.** Porter's `dependencies` DAG is complex
   enough that v2 is a rewrite; Timoni punts entirely to runtime (cluster
   ConfigMaps). Build-time CEL refs (`${sources.<id>.values.<field>}`)
   would be a genuine improvement over both.

## Proposed v1 shape

```yaml
# Simple version string. NOT apiVersion: akua.tech/v1beta1.
# Kustomize spent ~7 years stuck on v1beta1. Helmfile went with
# plain `version: "1"` for their 1.0. We'll do the same.
version: "1"

# Optional. Defaults to Package. Present only to match the familiar
# apiVersion/kind/metadata/spec shape without pretending to be a K8s CR
# (which would confuse users trying to `kubectl apply -f package.yaml`).
kind: Package

metadata:
  name: hello-package
  version: 0.1.0
  description: Optional human-readable description.

# Separate schema file — already conventional. Keeps the YAML focused
# on composition; all customer-input definition stays in JSON Schema.
schema: ./values.schema.json

# ── The composition layer ──────────────────────────────────────────────

sources:
  - id: app                        # Stable handle. Used for alias + CEL refs.
    engine: helm                   # Which engine block to consume below.
    helm:                          # helm-engine block (typed fields, not raw chart.*)
      repo: https://charts.bitnami.com/bitnami
      # For OCI: repo: oci://ghcr.io/acme/charts
      chart: nginx
      version: 18.1.0
    values:                        # Typed YAML, NOT a string-of-YAML.
      replicaCount: 1

  - id: custom
    engine: kcl
    kcl:                           # kcl-engine block
      entrypoint: ./app.k
      # Chart version for the generated subchart wrapper:
      version: 0.1.0

  - id: stack
    engine: helmfile
    helmfile:                      # helmfile-engine block
      path: ./helmfile.yaml
      version: 0.1.0

# ── Hierarchical axis (dev / staging / prod) ───────────────────────────
# Inspired by helmfile's `environments:`. Selected at CLI via
# `akua build --env prod`. Values merge on top of each source's `values:`
# using the same deep-merge-maps / replace-arrays semantics as Helm.

environments:
  default: {}                      # Always present, always empty by default.
  prod:
    sources:
      app:
        values:
          replicaCount: 3

# ── Orthogonal axis (features / mixins) ───────────────────────────────
# Inspired by kustomize's Components. Can be combined freely in any
# order: `akua build --env prod --component tls,monitoring`.

components:
  tls:
    sources:
      app:
        values:
          tls:
            enabled: true
  monitoring:
    # A component may ADD sources to the umbrella, not just patch values.
    sources:
      - id: prometheus
        engine: helm
        helm:
          repo: https://prometheus-community.github.io/helm-charts
          chart: prometheus
          version: 25.x
```

## Design decisions — up for review

| Decision | Proposed | Alternative | Rationale |
|---|---|---|---|
| Version discriminator | `version: "1"` | `apiVersion: akua.tech/v1alpha1` | Kustomize stuck on v1beta1 for 7 years; Helmfile went plain. Akua is a build file, not a K8s CR. |
| `kind: Package` | Optional, defaults to `Package` | Drop entirely | Familiarity without kubectl-apply confusion. Cheap to keep. |
| Per-engine blocks (`helm:`, `kcl:`, `helmfile:`) | Yes | Keep `chart:` + engine-specific flags | Fixes the KCL shoehorn. Every serious tool has this shape. |
| `version:` instead of `targetRevision:` | Yes | Keep Argo terminology | Argo's word only makes sense for Argo. Helm/KCL/helmfile all use `version`. |
| Environments + components as two axes | Both upfront | Only environments (defer components) | Kustomize added components late → painful retrofit. Cost is ~nothing now. |
| `${sources.<id>.values.<field>}` in CEL | Yes | No cross-source refs | Real improvement over Porter's DAG + Timoni's runtime-lookup. Natural extension of existing CEL. |
| Values are typed YAML only | Yes | Allow string-YAML as alternative | ArgoCD shipped `values` (string) then `valuesObject` (typed); the string form was the #1 complaint. Skip the mistake. |
| Formal `parameters:` block in package.yaml | **No** — stays in `values.schema.json` via `x-user-input` | Duplicate in package.yaml | Already works. Don't duplicate. |
| Multi-file (`package.d/`) | Deferred | Support upfront | Single file scales surprisingly far (both helmfile and kustomize confirm). Add when users hit the wall. |

## What we explicitly reject

- **Flux's `spec.chart.spec.*` double-nest.** Historical artifact; universally complained about.
- **Porter's mixin-as-binary contract.** Ecosystem fragmented; every mixin reinvents its own schema.
- **Timoni's CUE-only schema.** CUE learning cliff is cited in every HN thread. Akua uses JSON Schema (easier onramp, mature tooling).
- **Helmfile's Go-template-the-YAML pattern.** Non-determinism via `now`/`exec` is exactly what Akua's determinism thesis rules out. CEL expressions inside `x-input` are the principled alternative.
- **Multiple patch dialects (Kustomize's mistake).** One merge semantic: deep-merge maps, replace arrays.

## Migration path

One-time breaking change. No published packages exist yet, so cost is ~zero.
We'd:

1. Add a `akua migrate-v1` subcommand that converts the current format
   mechanically.
2. Ship with both readers for one release so anyone with an in-flight
   package.yaml isn't blocked.
3. Remove the v0 reader in the release after — by which point migrate
   has been available for weeks.

## Open questions for you

1. **Do we want `kind: Package` at all, or drop it?** I lean "keep, optional"
   for future-proofing (e.g., a future `kind: Bundle` that composes many
   packages). But it's cosmetic.
2. **Components block — values-only patches, or can components add whole
   sources?** Proposed allows both (see the `monitoring` example above).
   Kustomize's equivalent (`component`) does allow adding resources, so
   precedent supports the broader power.
3. **CEL cross-source refs — resolve at schema-extraction time or a separate
   build-time pass?** Leaning "extract-time" so `akua preview` can show
   resolved values with cross-source data. Implementation detail but affects
   the mental model in docs.
4. **`environments:` default vs. no-default?** Currently proposed to always
   have `default: {}`. Alternative: no environment unless `--env` is passed.
   First is friendlier; second is more explicit.
5. **File structure — allow `package.d/*.yaml` for splitting, or require
   single file until users complain?** Proposed to defer. Easy to add later;
   annoying to remove.

## What's not in this doc

- **Secret-field routing** (`x-secret: true` → Sealed Secrets / ESO /
  Infisical). Orthogonal — stays in the schema file alongside other
  `x-*` extensions.
- **`uniqueIn` registry protocol.** Also orthogonal; the package.yaml
  shape doesn't change.

---

**Review:** comments welcome on any of the "Design decisions" table rows or
the five open questions. Once agreed, implementation is probably 2–3 focused
days: migrate reader, new types + parser, CLI flag wiring, doc sync,
migrate command.
