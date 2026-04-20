# Use Cases & User Journeys

End-to-end flows for the three roles in an akua-powered ecosystem, plus concrete reconciler wiring for the two common install models.

> **Authoritative authoring shape** lives in [`package-format.md`](./package-format.md). **Runnable examples** live in [`examples/`](./examples/). This doc is the operational "how it flows" reference.

---

## Roles

| Role | Who | Does what | Uses |
|---|---|---|---|
| **Package author** | Platform team, ISV, upstream maintainer | Writes `Package.k` in KCL, optionally composing Helm / KCL / Kustomize / RGD sources. Publishes signed OCI artifact. | `akua` CLI / Package Studio / CI |
| **Operator** | Customer ops / platform engineer | Authors `App.k` (or `app.yaml`) referencing a Package digest. Commits to git. Reviews rendered output. | ArgoCD / Flux / kro / Helm release |
| **Customer** | End user of a hosted product | Fills an install form or approves a PR. Never touches typed code. | Install UI, review surface |

In a managed-SaaS model, operator and customer collapse into "the tenant," and the platform operator owns the reconciler. In self-hosted or CLI flows, all three roles may be the same person.

---

## Author flow

```
    Author                      akua CLI / CI                  OCI registry
    ──────                      ─────────────                  ────────────

    Package.k        ───┐
    (imports engines,  │
     declares schema,  │      akua check         (syntax + types)
     wires outputs)    │      akua lint          (Regal + kcl lint)
                       ├──▶   akua test          (*_test.rego + test_*.k)
    sources/           │      akua render --plan (dry run with sample inputs)
    (helm / kcl /      │      akua fmt --check
     kustomize / ...)  │
                       │      akua publish       ──────────────▶  signed + attested
    akua.toml / .sum   ─┘      (cosign + SLSA v1)                   OCI artifact
```

Every verb honors [`cli-contract.md`](./cli-contract.md): `--json`, `--plan`, typed exit codes, structured errors, determinism.

---

## Install flow — two models

Packages are designed to work with either model. Authors don't commit to one. The choice lives on the install side.

### Model A — shared chart, per-install values (cheap fan-out)

One OCI digest, many deploys, per-tenant values resolved at deploy-time.

```
    Package digest    ──▶   one OCI artifact   ──▶   ArgoCD Application per tenant
                                                     (releaseName + inputs differ)
```

Consumers: ArgoCD Helm source, Flux `HelmRelease`, `helm install`. `akua render` executes on commit to produce per-tenant rendered manifests, committed to the deploy path (compiled GitOps); or the reconciler does the templating itself against the shared chart.

When Model A works:
- ✅ Late-bindable engines (Helm templates, RGD with deploy-time CEL).
- ✅ Per-tenant differences fit within values (inputs).

When Model A breaks down:
- ❌ Early-binding engines (KCL, Kustomize) where all inputs must resolve at build time.
- ❌ Per-tenant template logic varies (not just values) — one tenant gets an extra sidecar.
- ❌ Content-addressable per-install guarantees required ("prove tenant X runs sha256:Y with baked-in inputs").

### Model B — per-install chart, values baked in (escape hatch)

One OCI digest per install. `akua render` runs the full pipeline with that tenant's inputs and pushes a sealed artifact.

```
    Package + tenant inputs   ──▶   akua render + publish   ──▶   tenant-specific OCI digest
                                                                  (chart@sha256:xxx)
```

Consumers: ArgoCD Application pointing at the sealed digest. Works with any engine (KCL, Kustomize, Helmfile) because binding happens pre-deploy.

---

## Reconciler wiring

### ArgoCD — Model A

```yaml
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: acme-web-tenant-acme
  namespace: argocd
spec:
  project: tenants
  source:
    repoURL: oci://ghcr.io/acme/charts
    chart: acme-web
    targetRevision: "1.0.0"
    helm:
      releaseName: acme
      values: |
        subdomain: acme.apps.example.com
        adminEmail: ops@acme.corp
  destination:
    server: https://kubernetes.default.svc
    namespace: tenant-acme
  syncPolicy:
    automated: { prune: true, selfHeal: true }
```

Update propagation: author publishes v1.1 → ArgoCD Image Updater / Renovate bumps `targetRevision` in N Applications → each re-syncs.

### ArgoCD — Model B

```yaml
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: acme-web-tenant-acme
spec:
  source:
    repoURL: oci://ghcr.io/acme/installs
    chart: acme-web-tenant-acme
    targetRevision: "sha256:abc..."
  destination:
    namespace: tenant-acme
```

Each tenant pins to a sealed digest. Updates are per-tenant re-renders.

### Flux, kro, Helm release lifecycle

See [`docs/embedded-engines.md`](./embedded-engines.md) for reconciler-consumption matrix and [`examples/`](./examples/) for working specimens.

---

## Self-hosted / CLI-only flow

Same Package, same `akua` binary, no install UI.

```
Developer:
  akua dev                       # sub-second hot-reload against local cluster
  akua render --inputs my.yaml   # produce raw manifests
  akua publish --to oci://myregistry/mychart   (optional)

Developer or ops:
  helm install mychart oci://myregistry/mychart --values my-inputs.yaml

Or via reconciler:
  kubectl apply -f argocd-application.yaml
```

`akua` is a build tool here. No hosting platform, no install UI. The output is a vanilla Helm chart (or raw manifests, or RGD, or whatever format the Package's `outputs` declared) — any OCI-aware consumer works.

---

## What akua does NOT do at deploy time

- ❌ Run any code in the customer's cluster.
- ❌ Require a controller installed for akua packages.
- ❌ Mutate manifests after they leave the published artifact.
- ❌ Require a specific reconciler — Flux, raw `helm install`, `kubectl apply`, or any OCI-aware deployer works.

All akua magic finishes **before the artifact digest is computed**. Once published, the artifact is inert: just bytes with a sha256 hash, reconciled by whichever system the customer chose.
