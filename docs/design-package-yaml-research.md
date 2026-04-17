# Research notes — package.yaml format prior art

> Companion to [`design-package-yaml-v1.md`](./design-package-yaml-v1.md).
> Raw findings from three research passes (helmfile + kustomize, Flux
> HelmRelease + ArgoCD Application, Timoni + Porter). Keep around so we
> don't re-research when the next breaking change comes up.

## Helmfile + Kustomize

**Schema shape.** Helmfile: flat top-level keys, no `apiVersion`/`kind`,
uses a `.gotmpl` file extension to opt into templating. Kustomize: full
CRD-style `apiVersion: kustomize.config.k8s.io/v1beta1` + `kind:
Kustomization`. Kustomize has been on `v1beta1` for ~7 years — the
"declare beta, regret forever" trap.

**Minimal useful field set.** Helmfile's real core is 4 keys:
`environments`, `releases`, `repositories`, `helmDefaults`. Everything
else is sugar. Kustomize's core is 2: `resources` + one of
`patches`/`components`. But the *convenience* fields (`namePrefix`,
`commonLabels`, `images`, `replicas`) are the #1 most-cited kustomize
wins — one line replaces a patch file.

**Environments / overlays.** Helmfile keeps everything in one file with
an `environments:` map + `bases:` for inheritance. Kustomize uses
directory-per-overlay. Kustomize later added
[Components (KEP-1802)](https://github.com/kubernetes/enhancements/blob/master/keps/sig-cli/1802-kustomize-components/README.md)
because stacked inheritance doesn't compose — sibling overlays can't
modify a shared parent without GVKN conflicts.

**Values merge.** Helmfile: deep-merge maps, arrays replaced wholesale.
Kustomize: strategic merge + JSON-6902 + the newer unified `patches:`
([#4376](https://github.com/kubernetes-sigs/kustomize/issues/4376)) —
consolidating from three fields to one was a retrospective win.

**User complaints.**
- Kustomize: patch-field fragmentation
  ([#5052](https://github.com/kubernetes-sigs/kustomize/issues/5052));
  `vars` → `replacements` migration broke workflows
  ([#4701](https://github.com/kubernetes-sigs/kustomize/issues/4701));
  component ordering ambiguity
  ([#5172](https://github.com/kubernetes-sigs/kustomize/issues/5172));
  permanent-`v1beta1`.
- Helmfile: two-pass rendering confusion; `.yaml` vs `.gotmpl`
  ambiguity forced a breaking v1 change.

**apiVersion evolution.** Helmfile 1.0 (May 2025) was their first major
bump; two-year soft-migration window. Kustomize still at `v1beta1`.

**File structure.** Helmfile: single `helmfile.yaml`, optional
`helmfile.d/` with explicit `bases:`. Kustomize: per-directory
`kustomization.yaml` with explicit `resources:`/`components:`/`patches:`
references (no globs — widely praised even by critics).

Sources: [Kustomization reference](https://kubectl.docs.kubernetes.io/references/kustomize/kustomization/),
[Helmfile v1 announcement](https://github.com/helmfile/helmfile/discussions/1912),
[towards-1.0 proposal](https://helmfile.readthedocs.io/en/stable/proposals/towards-1.0/),
[Helm #3486 — deep merge values](https://github.com/helm/helm/issues/3486).

## Flux HelmRelease + ArgoCD Application

Both use standard `apiVersion/kind/metadata/spec`. Divergence is
entirely under `spec`.

**Flux HelmRelease** separates the chart *source*
(`HelmRepository`/`GitRepository`/`OCIRepository` CRs) from the release
via `spec.chart.spec.sourceRef`. Fully normalized. The
`spec.chart.spec.*` double-nest is universally complained about — a
historical artifact.

**ArgoCD Application** inlines the source: `spec.source.repoURL` is a
raw URL; chart-type detected via presence of `spec.source.chart`.
Denormalized.

**Argo multi-source** (GA'd in 2.8): `spec.sources: []`, each entry the
same shape, plus **`ref`** on a source + `$ref` in `valueFiles` — e.g.
chart pulled from Helm repo with `valueFiles: ["$values/env/prod.yaml"]`
pulling from a Git source tagged `ref: values`. This is the proven shape
for cross-source composition in a CRD.

**Argo's dual API for Helm values** is the cautionary tale: shipped
`values` (string-of-YAML) and `helm.parameters` (typed key/value), THEN
added `valuesObject` (typed YAML object) because strings-of-YAML sucked.
Three ways to do the same thing. Do not repeat this mistake.

**For a build-time tool** (Akua), the translatable patterns are:
- Argo's inlined flat source beats Flux's normalized cross-CR indirection
  (a build file has no cluster to resolve refs in).
- Argo's `sources[]` with `ref`/`$ref` handles if Akua ever composes
  inputs from multiple places.
- Typed YAML only — never string-YAML.
- Flux's `valuesFrom` with `kind: ConfigMap/Secret` is a cluster-native
  concept with no analog in a build file.

## Timoni + Porter

**Timoni bundle** (CUE-based): flat schema with `apiVersion: v1alpha1`,
`name`, `instances: {[name]: {module:{url,version,digest}, namespace,
values}}`. No `kind`. Module = directory of `.cue` files with a `#Config`
schema for validation.

**Porter** (YAML): `schemaVersion`, `name`, `version`, `description`,
`mixins[]`, `parameters[]`, `credentials[]`, `outputs[]`, and action
blocks `install/upgrade/uninstall[]`. Parameters/credentials are
**arrays of typed objects** with `name/type/default/sensitive/env/path/applyTo`.

**Mixins (Porter).** Go binaries implementing a JSON-over-stdin
contract. Declared as `mixins: [exec, helm3, terraform, kubernetes,
...]`. **Did not scale well** —
[#897 naming rules](https://github.com/getporter/porter/issues/897),
[#2350 user-agent](https://github.com/getporter/porter/issues/2350),
[#2240 CLI lib churn](https://github.com/getporter/porter/issues/2240),
[#256 HTTP mixin stalled 5+ years](https://github.com/getporter/porter/issues/256).
Fewer broader engines beat many narrow mixins.

**Cross-source wiring.**
- Porter has an explicit `dependencies:` block + `source: { dependency:
  X, output: Y }` on parameters. Dependency graph is correct but complex
  enough that [dependencies-v2 is a full rewrite](https://github.com/getporter/porter/tree/main/docs/content/docs/development).
- Timoni explicitly has **no cross-instance wiring**: "Bundle instances
  cannot directly reference outputs from other instances — all
  cross-instance data must flow through the Runtime layer"
  ([bundle-runtime docs](https://timoni.sh/bundle-runtime/)). A cop-out,
  but genuinely simpler.

**For Akua**: static build-time CEL refs
(`${sources.<id>.values.<field>}`) sit between Porter's DAG complexity
and Timoni's runtime punt. Neither tool does this well.

**Parameters / user inputs.**
- Porter: explicit typed array. Very Stripe-like, readable without
  tooling.
- Timoni: CUE `#Config` schema. Superior validation but requires CUE
  literacy.

**Applicable to Akua:**
- Porter's typed-parameter shape (we already have something similar in
  `x-user-input`).
- Timoni's flat `instances` map keyed by name (great for umbrella
  composition).
- Timoni's OCI-distributable module refs (`url + version + digest`).

**Reject:**
- CNAB invocation images (Porter). Akua renders at build-time.
- Mixin-as-binary contract (Porter). Use typed engine interface.
- Runtime-only cross-instance wiring (Timoni). Resolve at build time.

---

**Top lessons condensed:**

1. Flat schema + explicit `version: "1"` string. Avoid CRD framing unless
   K8s-native.
2. Two composition axes from day 1: hierarchical environments + orthogonal
   components. Kustomize took ~4 years to bolt on.
3. One patch/merge dialect. Deep-merge maps, replace arrays.
4. Per-engine sub-blocks under each source (not shoehorned `chart.*`).
5. Typed YAML values only. No string-of-YAML fields anywhere.
6. No implicit env-var / shell expansion. Require explicit references.
7. Single file by default. `package.d/` for scale. No globs.
8. Ship deprecation-with-alternatives as policy before 1.0.
9. Build-time cross-source refs (`${sources.<id>.values.<field>}`) — neither
   Porter's DAG nor Timoni's runtime-lookup got this right. Akua can lead.
