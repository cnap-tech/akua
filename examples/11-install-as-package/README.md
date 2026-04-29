# 11-install-as-package

Composes a third-party Akua package (`./upstream/`) and applies a
tenant overlay, drops a kind, and appends extras — the
install-as-Package shape. Demonstrates the natural call form
`upstream.render(upstream.Input{...})`: typed inputs at the consumer
call site, no path strings in user code.

## Layout

| file | purpose |
|---|---|
| `upstream/package.k` | Sibling Akua package — Deployment + Service + PodDisruptionBudget. Authored as a normal Package; nothing about it is install-aware. |
| `akua.toml` | Dep alias `upstream = { path = "./upstream" }` for `akua tree` + lock-time validation. |
| `package.k` | The install: `upstream.render(...)`, overlay tenant label, drop PDB, append a ConfigMap. |
| `inputs.example.yaml` | Per-install inputs (tenant, app, replicas). |
| `rendered/` | Reference output (3 files: Deployment, Service, ConfigMap). |

## Render

```sh
akua render --out ./rendered
```

## The install pattern

```kcl
import pkgs.upstream as upstream

_up = upstream.render(upstream.Input { appName = ..., replicas = ... })

# Overlay
_patched = [r | {metadata.labels = {"install.cnap.tech/tenant" = input.tenant}} for r in _up]

# Filter
_filtered = [r for r in _patched if r.kind != "PodDisruptionBudget"]

# Extras
_extras = [{apiVersion = "v1", kind = "ConfigMap", ...}]

resources = _filtered + _extras
```

The import lands a synthesized stub that owns a `render` lambda and
re-exports upstream's schemas — KCL type-checks `upstream.Input{...}`
at the call site (typos surface as compile errors, not as runtime
worker traps). The mechanism mirrors `import charts.<name>` for Helm
charts; see [docs/package-format.md](../../docs/package-format.md)
for the full shape.
