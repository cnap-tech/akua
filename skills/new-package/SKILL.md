---
name: new-package
description: Scaffold a new akua Package in KCL with a typed input schema and Helm/KCL/kro sources. Use when the user wants to start a new cloud-native package, create a new app definition, set up akua for a project, or compose multiple upstream sources into one reusable package.
license: Apache-2.0
---

# Create a new akua Package

An akua Package is a typed, reusable definition authored in KCL. One Package is published to OCI once; many Apps reference it with different inputs. This skill walks through scaffolding one from scratch.

## Prerequisites

- `akua` CLI installed (`akua --version` should succeed)
- A target directory for the new package
- Decision on the source engine(s) needed: Helm chart(s), KCL modules, kro RGDs, kustomize bases, or a mix

## Steps

### 1. Scaffold

```sh
akua init <package-name>
cd <package-name>
```

This creates:

- `package.k` — the KCL Package definition with a starter schema
- `inputs.example.yaml` — sample inputs satisfying the schema
- `.akua/` — metadata directory
- `README.md`

### 2. Add source engines

For each external source, use `akua add`. This generates a typed KCL subpackage under `./sources/` so you get autocomplete + validation on the source's native values.

Helm chart:

```sh
akua add chart oci://ghcr.io/cloudnative-pg/charts/cluster --version 0.20.0
```

kro RGD:

```sh
akua add rgd oci://pkg.example.com/glue-rgd:1.0
```

Kustomize base:

```sh
akua add kustomize ./local/overlay
```

Another KCL package:

```sh
akua add kcl oci://ghcr.io/kcl-lang/k8s --version 1.31.2
```

### 3. Edit the schema

Open `package.k`. The top defines the public input contract:

```python
schema Input:
    """What callers of this package must provide."""
    appName:  str
    hostname: str
    replicas: int = 3
    database: { user: str = "app" }

input: Input
```

Rules for good schemas:

- Required fields have no default (`appName`, `hostname` above).
- Optional fields get sensible defaults (`replicas: int = 3`).
- Nested schemas for structured inputs (`database`).
- Descriptions go in docstrings — agents and humans both read them.
- No cross-field logic in the schema — that goes in the package body.

### 4. Wire sources to schema

Below the schema, call the engine functions with the typed inputs:

```python
import akua.helm
import charts.cnpg as cnpg

_pg = helm.template(cnpg.Chart {
    values = cnpg.Values {
        cluster.name      = "${input.appName}-pg"
        cluster.instances = 3
    }
})

resources = _pg
```

Each call returns a list of typed Kubernetes resources. Concatenate them with `[*_pg, *_app, ...]`.

### 5. Declare outputs

```python
outputs = [
    {
        kind:   "RawManifests"
        target: "./"
    }
]
```

Most packages have one `RawManifests` output. Advanced: emit `ResourceGraphDefinition` for kro runtime late-binding, or `HelmChart` for Helm-release lifecycle, or multiple in parallel. See [docs/cli.md — akua render](../../docs/cli.md#akua-render).

### 6. Validate

```sh
akua lint
```

Catches: schema errors, unresolved source references, policy violations (if `--policy` is set).

### 7. Render with sample inputs

```sh
akua render --inputs inputs.example.yaml --out ./rendered
```

Inspect `./rendered/` — this is the exact YAML that would deploy. Committable to git.

### 8. Dev loop (optional)

If you have a local Kubernetes available:

```sh
akua dev
```

Opens `http://localhost:5173`, applies rendered manifests to a kind cluster, hot-reloads on every edit.

## Expected output

After `akua render`, `./rendered/` contains well-formed Kubernetes YAML, one file per resource, sorted by `kind/name`. The output is byte-deterministic: the same inputs always produce identical bytes.

## Failure modes

- **`E_SCHEMA_INVALID`** — schema definition has invalid KCL. The error includes line and field; fix the schema.
- **`E_SOURCE_UNRESOLVED`** — a source reference in `akua add` failed to fetch. Check the OCI ref; verify credentials with `akua whoami`.
- **`E_POLICY_DENY`** (exit 3) — rendered manifests violate the configured policy tier. The error lists the failing rule and a suggested fix.
- **`E_RENDER_FAILED`** — an engine function (helm.template, rgd.instantiate) raised an error. Usually a values-validation failure; check the source engine's own schema.

## Next steps

- Publish: use the [publish-signed](../publish-signed/SKILL.md) skill.
- Set up a CI gate: use the [diff-gate](../diff-gate/SKILL.md) skill.
- Apply a policy tier: use the [apply-policy-tier](../apply-policy-tier/SKILL.md) skill.

## Reference

- Full CLI: [docs/cli.md](../../docs/cli.md)
- Examples: [examples/](../../examples/)
- Package format spec: `docs/package-format.md` (forthcoming)
