# 11-install-as-package

Composes a third-party Akua package (`./upstream/`) and applies a
tenant overlay, drops a kind, and appends extras — the install-as-Package
shape. Demonstrates the full power of `pkg.render` returning a real
list: list-comprehension overlay + filter + concatenation all work
in plain KCL.

## Layout

| file | purpose |
|---|---|
| `upstream/package.k` | Sibling Akua package — Deployment + Service + PodDisruptionBudget. Authored as a normal Package; nothing about it is install-aware. |
| `akua.toml` | Path-dep on `upstream` for `akua tree` + lock-time validation. |
| `package.k` | The install: `pkg.render` the upstream, overlay tenant label, drop PDB, append a ConfigMap. |
| `inputs.example.yaml` | Per-install inputs (tenant, app, replicas). |
| `rendered/` | Reference output (3 files: Deployment, Service, ConfigMap). |

## Render

```sh
akua render --out ./rendered
```

## The install pattern

```kcl
_up = pkg.render(pkg.Render { path = "./upstream", inputs = {...} })

# Overlay
_patched = [r | {metadata.labels = {"install.cnap.tech/tenant" = input.tenant}} for r in _up]

# Filter
_filtered = [r for r in _patched if r.kind != "PodDisruptionBudget"]

# Extras
_extras = [{apiVersion = "v1", kind = "ConfigMap", ...}]

resources = _filtered + _extras
```

Three plain KCL operations on a real list — no special install vocabulary.
The upstream package authors a normal `package.k`; the install is just
a Package that uses `pkg.render` like any other engine call.
