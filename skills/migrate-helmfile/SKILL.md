---
name: migrate-helmfile
description: Convert an existing helmfile.yaml into a typed akua Package written in KCL. Use when migrating from Helmfile to akua, consolidating multi-release orchestration into a single package, replacing Helmfile's untyped templating with a typed schema, or moving a platform repo from helmfile apply to GitOps render-at-CI.
license: Apache-2.0
---

# Migrate from helmfile to an akua Package

Helmfile orchestrates multiple Helm releases with templated values. akua replaces this with a single typed KCL Package that calls `helm.template()` per source. The migration is mechanical — each Helmfile release becomes one `helm.template` call; templated values become typed schema inputs.

## When to use

- Existing `helmfile.yaml` managing 2+ Helm releases
- Team wants typed inputs, structural diffs, signed artifacts
- Moving from `helmfile apply` (imperative) to GitOps (render-at-CI + Argo/Flux apply)

## Steps

### 1. Scaffold a new akua Package

```sh
akua init <package-name>
cd <package-name>
```

### 2. Inventory the Helmfile

Read the source `helmfile.yaml`. Identify:

- Each `releases[].chart` and `releases[].version` → becomes a `helm.template` call
- Each `releases[].values` → becomes input mapping in the KCL call
- Each `templateValue` / Go template expression → becomes a schema field + KCL expression
- Each `helmDefaults` → becomes shared defaults in KCL

Example source:

```yaml
# helmfile.yaml
releases:
  - name: cnpg
    chart: cnpg/cluster
    version: 0.20.0
    values:
      - cluster:
          name: "{{ .Values.appName }}-pg"
          instances: 3
  - name: webapp
    chart: ./charts/webapp
    values:
      - replicaCount: "{{ .Values.replicas | default 3 }}"
        ingress:
          hostname: "{{ .Values.hostname }}"
```

### 3. Derive the schema

Every Go-template expression `{{ .Values.X }}` becomes a schema field:

```python
schema Input:
    appName:  str
    hostname: str
    replicas: int = 3

input: Input
```

Defaults that were in Helmfile's `| default 3` move into the schema's `int = 3`.

### 4. Add each Helm source

For each release:

```sh
akua add chart oci://ghcr.io/cloudnative-pg/charts/cluster --version 0.20.0
akua add chart ./charts/webapp
```

This generates typed `Chart` and `Values` subpackages, so you get autocomplete on chart values.

### 5. Replace releases with helm.template calls

```python
import akua.helm
import charts.cnpg    as cnpg
import charts.webapp  as webapp

_pg = helm.template(cnpg.Chart {
    values = cnpg.Values {
        cluster.name      = "${input.appName}-pg"
        cluster.instances = 3
    }
})

_app = helm.template(webapp.Chart {
    values = webapp.Values {
        replicaCount     = input.replicas
        ingress.hostname = input.hostname
    }
})

resources = [*_pg, *_app]

outputs = [
    {
        kind:   "RawManifests"
        target: "./"
    }
]
```

### 6. Migrate transforms

If the helmfile uses `transformers:` (including the KRM/KCL transformer), convert each transform into a KCL `postRenderer` lambda:

```python
_app = helm.template(webapp.Chart {
    values = webapp.Values { ... }
    postRenderer = lambda r: dict -> dict {
        r.metadata.labels |= {"team": "payments"}
        r
    }
})
```

### 7. Render and verify

```sh
akua render --inputs inputs.yaml --out ./rendered
```

Diff the output against the original Helmfile's `helmfile template` output:

```sh
helmfile template > /tmp/before.yaml
akua render --inputs inputs.yaml --stdout > /tmp/after.yaml
diff /tmp/before.yaml /tmp/after.yaml
```

Expected: semantic equivalence. Cosmetic differences (whitespace, key ordering) are acceptable; structural differences are a migration bug.

### 8. Swap reconciler path

Helmfile was reconciling via `helmfile apply`. With akua, render at CI and commit raw YAML; let ArgoCD/Flux reconcile. Update your deploy pipeline:

- Remove `helmfile apply` from CI
- Add `akua render --out deploy/` to CI; commit the deploy/ directory
- Configure Argo/Flux to sync from the deploy/ path

### 9. Decommission Helmfile

Once the akua Package renders identically and a production run has succeeded, delete `helmfile.yaml` and remove `helmfile` from CI. Keep the git history for reference.

## What doesn't translate directly

- **Helmfile hooks** (`hooks.prepare`, `hooks.postsync`) — akua has no equivalent by design; use Argo/Flux PreSync/PostSync hooks or a Runbook.
- **Helmfile's `needs:` ordering** — replaced by Kubernetes' level-triggered reconciliation (works for most cases) or by emitting `kro` RGD output for explicit runtime late-binding.
- **Helmfile's `selectors` for partial apply** — use named outputs and per-environment inputs instead; see [examples/03-multi-env-app](../../examples/03-multi-env-app/).

## Failure modes

- **Rendered output drifts from Helmfile's** — usually a values-mapping miss. Compare native Helm values before and after; often a nested field got flattened incorrectly.
- **Chart-local `requirements.yaml` missing** — akua fetches chart dependencies automatically; Helmfile's `helmfile deps` step is no longer needed.
- **Secrets referenced inline in helmfile values** — use `akua secret` refs instead; never put raw secret values in the Package.

## Reference

- [cli.md — akua add](../../docs/cli.md#akua-add)
- [examples/02-webapp-postgres](../../examples/02-webapp-postgres/) — canonical multi-source example
- [new-package](../new-package/SKILL.md) — if starting fresh instead of migrating
