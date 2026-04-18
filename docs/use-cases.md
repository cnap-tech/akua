# Use Cases & User Journeys

End-to-end flows for the three roles in an Akua-powered ecosystem, plus
concrete ArgoCD wiring for both the shared-chart and per-customer-chart
OCI models.

> **For design rationale, see [`design-notes.md`](./design-notes.md).**
> This doc is the operational "how it flows" reference.

---

## Roles

| Role | Who | Does what | Uses |
|---|---|---|---|
| **Package author** | PaaS builder, platform team, ISV | Writes `package.yaml` + JSON Schema + engine source. Publishes chart to OCI. | Chart Studio IDE / CLI / CI |
| **Operator** | Customer ops / DevOps | Installs + maintains the chart in their cluster. Binds chart digest to ArgoCD Applications. | ArgoCD / Flux / helm |
| **Customer** | End user of the hosted product | Fills an install form. Never touches YAML. | Install UI (web) |

In CNAP's managed-SaaS model, operator and customer collapse into "the
tenant," and CNAP-internal ops owns ArgoCD. In self-hosted or CLI
flows, all three roles may be the same person.

---

## Authoring flow (package author)

```
┌────────────────────────────────────────────────────────────────┐
│  Author's machine / Chart Studio IDE                           │
├────────────────────────────────────────────────────────────────┤
│                                                                 │
│  package.yaml ──┐                                              │
│  values.schema.json ──┤                                        │
│  sources/ (helm / kcl / kustomize / helmfile) ──┤              │
│                                                  ▼             │
│              akua preview (live CEL + transforms via WASM)     │
│              akua lint                                         │
│              akua tree                                         │
│              akua render --inputs ...   (helm template)        │
│              akua test                                         │
│                                                  │             │
│                                                  ▼             │
│              akua build   →  dist/chart/ (Chart.yaml + templates)│
│              akua publish  →  oci://reg/chart@sha256:...       │
│                                                                 │
└────────────────────────────────────────────────────────────────┘
```

### Concrete example — minimal package

```yaml
# package.yaml
name: acme-web
version: 1.0.0
engine: helm                  # or: kcl, kustomize, helmfile
sources:
  - id: app
    chart:
      repoUrl: oci://ghcr.io/acme/charts
      chart: web
      targetRevision: 2.3.1
    values:
      replicaCount: 1
```

```json
// values.schema.json
{
  "type": "object",
  "properties": {
    "subdomain": {
      "type": "string",
      "title": "Subdomain",
      "x-user-input": { "order": 10 },
      "x-input": {
        "cel": "value.lowerAscii() + '.apps.example.com'",
        "uniqueIn": "tenant.hostnames"
      }
    },
    "adminEmail": {
      "type": "string",
      "format": "email",
      "title": "Admin email",
      "x-user-input": { "order": 20 }
    }
  },
  "required": ["subdomain", "adminEmail"]
}
```

### Commands the author runs

| Command | What it does |
|---|---|
| `akua preview --inputs '…'` | Dry-run: evaluates CEL, prints resolved values |
| `akua tree` | Shows umbrella dep structure |
| `akua render --inputs '…'` | Full helm template; write rendered YAML |
| `akua lint` | Validates schema, checks engine source |
| `akua build --out dist/chart` | Writes the chart dir + `.akua/metadata.yaml` |
| `akua inspect --chart dist/chart` | Prints the `.akua/metadata.yaml` lineage |
| `akua publish --chart dist/chart --to oci://…` | `helm package` + `helm push`; returns OCI digest |
| `akua attest --chart dist/chart` | Emits SLSA v1 provenance predicate for cosign |

Publishing a SLSA-attested chart is a three-step flow:

```bash
akua build --package ./my-pkg --out dist/chart
akua publish --chart dist/chart --to oci://ghcr.io/acme/charts
# note the printed digest, then:
akua attest --chart dist/chart --out dist/attestation.json
cosign attest \
  --predicate dist/attestation.json \
  --type slsaprovenance1 \
  ghcr.io/acme/charts/my-pkg@sha256:<digest>
```

Same package version + same sources → same OCI digest (for engines that
respect determinism — see `design-notes.md` §11).

---

## Install flow — two OCI models

After a chart is published, there are two ways to deploy it per customer.
**Default to Model A. Use Model B only when necessary.**

### Model A — shared chart, per-install values (default, efficient)

One OCI digest serves all customers. Each customer's inputs become a
small `values.yaml` applied at deploy time.

```
┌────────────────┐     ┌───────────────────────┐     ┌──────────────┐
│  Chart         │     │  Per-customer values  │     │  ArgoCD      │
│  @sha256:abc   │     │  acme.yaml, etc.      │     │  Application │
│  (shared)      │     │  (computed by Akua    │     │  per install │
│                │     │   from form inputs)   │     │              │
└───────┬────────┘     └──────────┬────────────┘     └──────┬───────┘
        │                         │                         │
        │                         └──────────┬──────────────┘
        │                                    │
        ▼                                    ▼
   ┌────────────────────────────────────────────────┐
   │  Helm render at deploy time (in-cluster)       │
   │    chart + values → Kubernetes manifests       │
   └────────────────────────────────────────────────┘
```

**Customer flow (Model A)**

1. Customer opens install UI, sees form from `values.schema.json`.
2. Browser evaluates CEL live via WASM → shows resolved values as they
   type. Zero network round-trip.
3. Customer submits.
4. Server-side: Akua runs `applyInputTransforms` → produces a small
   resolved `values.yaml` (KBs).
5. CNAP creates an ArgoCD `Application` pointing at the shared chart +
   the customer's resolved values.

**ArgoCD `Application` (Model A)**

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
    targetRevision: "1.0.0"             # Helm chart version
    helm:
      releaseName: acme
      # Akua-resolved values inlined here.
      values: |
        subdomain: acme.apps.example.com
        adminEmail: ops@acme.corp
  destination:
    server: https://kubernetes.default.svc
    namespace: tenant-acme
  syncPolicy:
    automated: { prune: true, selfHeal: true }
```

**Update propagation (Model A)**

```
Author: akua publish v2  →  oci://reg/chart:1.0.1@sha256:def
                                              ▲
                                              │
ArgoCD Image Updater / Renovate bumps         │
`targetRevision: 1.0.1` in all Applications  │
                                              │
Each Application re-syncs with v2 chart + existing values.
Akua also re-evaluates CEL if the schema changed.
```

If v2 only adds optional fields → zero customer action. If v2 adds
required fields → customers prompted in the install UI to fill them
before sync proceeds.

**When Model A works**

✅ Chart uses helm engine (or any engine that produces late-bindable
templates).
✅ Per-customer customization fits within `values.yaml`.
✅ You want cheap fan-out (one build, N deploys).

**When Model A breaks down**

❌ Chart uses an early-binding engine (`kcl`, `kustomize`, `helmfile`)
where all inputs must be resolved at build time.
❌ Per-tenant template **logic** varies (not just values) — e.g., one
tenant gets an extra sidecar, another doesn't.
❌ You need content-addressable guarantees at per-customer granularity
("prove this tenant runs exactly sha256:xyz with these inputs baked in").

---

### Model B — per-customer chart, values baked in (escape hatch)

One OCI digest per install. Customer inputs are evaluated at build
time and baked into the chart. ArgoCD just pulls the fully-sealed chart.

```
                                         ┌───────────────────────┐
Customer A fills form ──► Akua builds ──►│ chart@sha256:aaa      │──► ArgoCD (A)
                                         └───────────────────────┘
                                         ┌───────────────────────┐
Customer B fills form ──► Akua builds ──►│ chart@sha256:bbb      │──► ArgoCD (B)
                                         └───────────────────────┘
```

**Customer flow (Model B)**

1. Customer opens install UI, fills form (same as A).
2. Server-side: Akua runs the **full pipeline** — CEL, engine render,
   umbrella assembly, OCI push — producing a per-customer chart digest.
3. CNAP creates an ArgoCD `Application` pointing at the customer's
   specific digest.

**ArgoCD `Application` (Model B)**

```yaml
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: acme-web-tenant-acme
  namespace: argocd
spec:
  project: tenants
  source:
    repoURL: oci://ghcr.io/cnap/installs
    chart: acme-web-acme
    targetRevision: "@sha256:aaa1234…"   # pin to the customer's digest
    # NOTE: no `helm.values` — everything is baked into the chart.
  destination:
    server: https://kubernetes.default.svc
    namespace: tenant-acme
```

**Update propagation (Model B)**

```
Author: akua publish v2  →  new BASE chart @sha256:def

For each customer install:
  Re-run the full Akua pipeline with their existing inputs
  against v2's base → new per-customer digest
  Bump the Application's targetRevision
```

More expensive than Model A (N builds per update), but each artifact
is fully sealed + content-addressed.

**When Model B is appropriate**

✅ Early-binding engines (KCL / kustomize / helmfile in some compositions).
✅ Per-tenant template logic varies, not just values.
✅ Compliance requires every install to have a signed, sealed, digest-addressable artifact.
✅ Extreme isolation: customer A's chart contents must not be visible to customer B.

**When Model B is wasteful**

❌ Helm-engine packages with pure-values-per-tenant customization → use A.
❌ High install volume with frequent chart updates → re-builds dominate cost.

---

## Decision table: which model?

| Situation | Default |
|---|---|
| Pure Helm chart with values-driven customization | **A** |
| Package mixes engines but everything compiles to late-bindable Helm templates | **A** |
| Package has a `kcl` / `kustomize` / `helmfile` component with early binding | **B** for that component (may still use A overall if it's a minor slice) |
| Per-tenant template **logic** varies (different sidecars, different ingresses per tenant) | **B** |
| Strong compliance / audit isolation per tenant | **B** |
| Thousands of installs, chart updates weekly | **A** (build cost of B multiplies) |
| Akua CLI / self-hosted developer with one cluster | Either works; A is simpler |

---

## Render / deploy time — what ArgoCD actually does

**Model A (shared chart)**

1. ArgoCD watches Application `spec.source` — sees `targetRevision: 1.0.0`.
2. Pulls `oci://ghcr.io/acme/charts/acme-web:1.0.0`.
3. Runs `helm template --values <spec.helm.values>` inside ArgoCD's
   repo-server (or via Helm CMP). Normal Helm semantics — hooks,
   release tracking, rollback all work.
4. Applies the rendered manifests to the cluster.

**Model B (per-customer chart)**

1. ArgoCD pulls `oci://.../acme-web-acme@sha256:aaa`.
2. Runs `helm template` with the chart's baked-in `values.yaml`.
   (Could also be `kubectl apply -f` if the chart ships pre-rendered
   manifests — both paths work.)
3. Applies to the cluster.

**In both cases: no custom runtime, no Akua agent in the cluster, no
CEL evaluation at deploy time.** The chart is plain Helm.

---

## Sample worked examples

### Example 1 — SaaS multi-tenant (Model A)

CNAP ships a "Postgres + API" product to 500 tenants.

- Author publishes `postgres-api@1.0.0` → one OCI digest.
- Tenants fill `{ subdomain, adminEmail, plan }`.
- Each tenant's Application has 3 values + pins to `1.0.0`.
- Author ships v1.1 → Renovate bumps Applications, 500 cheap re-syncs.

### Example 2 — KCL-authored chart (Model B)

An author prefers KCL to Helm templates.

- Package.yaml: `engine: kcl`, source is a `.k` program.
- KCL program reads `values.yaml` (the resolved inputs) at build time,
  emits rendered Kubernetes YAML.
- Akua wraps that YAML in a `Chart.yaml` + static `templates/` shell.
- Per tenant: Akua re-runs KCL with that tenant's inputs → per-tenant
  chart digest.
- Cheaper alternative: convert KCL to emit Helm **template** logic
  (sprig), producing a late-bindable chart — then back to Model A.

### Example 3 — Mixed engines, hybrid binding

Package has three components:

| Component | Engine | Binding |
|---|---|---|
| `redis` | helm (upstream chart) | late |
| `api` | helm (author-written) | late |
| `dashboard` | kustomize (pre-existing overlays) | early |

One umbrella chart. The `dashboard` component's manifests are
pre-rendered into `templates/dashboard-*.yaml`. The other two keep
their templates. Deploy-time Helm handles both. Model A works.

---

## The install UI ↔ Akua ↔ ArgoCD contract

```
                ┌─────────────────────────────────────┐
                │  Install UI (browser)               │
                │  - reads values.schema.json from    │
                │    the chart's OCI artifact         │
                │  - renders form                     │
                │  - @akua/core-wasm runs CEL live    │
                └──────────────────┬──────────────────┘
                                   │ POST inputs
                                   ▼
                ┌─────────────────────────────────────┐
                │  Install backend (CNAP)             │
                │  - Akua applyInputTransforms      │
                │  - Model A: write values.yaml       │
                │  - Model B: run full build pipeline │
                │  - Create ArgoCD Application        │
                └──────────────────┬──────────────────┘
                                   │ GitOps commit or API call
                                   ▼
                ┌─────────────────────────────────────┐
                │  ArgoCD                             │
                │  - pull chart from OCI              │
                │  - helm template                    │
                │  - apply to cluster                 │
                └─────────────────────────────────────┘
```

**All three layers share `akua-core`** (the same WASM module the
browser runs, the same Rust crate the backend calls, the same OCI
artifact ArgoCD pulls). No schema drift. No "what did the frontend
think?" debugging.

---

## Self-hosted / CLI flow

Same chart, same Akua, minus the multi-tenant install UI:

```
Developer:
  akua preview  (locally iterates on schema + inputs)
  akua build    (produces dist/chart/)
  akua publish --to oci://myregistry/mychart   (optional)

Developer or ops:
  helm install mychart oci://myregistry/mychart \
    --values my-inputs.yaml

or with ArgoCD:
  apply an Application pointing at the chart
```

Akua here is just a build tool. No CNAP, no install UI. The chart is
still a vanilla Helm chart anyone can consume.

---

## What Akua does NOT do (at deploy time)

- ❌ Run any code.
- ❌ Need a controller in the cluster.
- ❌ Mutate manifests after they leave the chart.
- ❌ Require ArgoCD specifically — Flux, raw `helm install`, or any
  OCI-aware deployer works.

All Akua magic finishes **before the chart digest is computed**. Once
published, the chart is inert: just bytes with a sha256 hash.
