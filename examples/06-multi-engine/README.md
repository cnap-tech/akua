# Example 06 — multi-engine

Four source engines composed in one Package, with per-source output routing. Demonstrates:

- `helm.template(...)` — a Helm chart consumed unchanged
- `kustomize.build(...)` — a Kustomize overlay on a base
- `rgd.instantiate(...)` — a kro RGD rendered offline (no controller)
- Inline KCL resources — just dicts, composed with the rest
- Named outputs — static resources routed to raw manifests, runtime-late-binding resources routed to an RGD for kro to reconcile

This is the "realistic monorepo Package" shape. Most production Packages have 2–5 sources; 4 is a reasonable teaching number.

## Layout

```
06-multi-engine/
├── akua.toml
├── akua.lock
├── package.k                      the Package; mixes Helm + Kustomize + kro + inline KCL
├── overlays/                      Kustomize overlay (local source)
│   ├── kustomization.yaml
│   └── servicemonitor-patch.yaml
├── inputs.yaml
└── README.md
```

## The Package, in pieces

### Imports

```python
import akua.helm
import akua.kustomize
import akua.rgd
import charts.webapp    as webapp
import rgds.platform    as platform
```

`charts.webapp` resolves to an OCI-published Helm chart via `akua.toml`. `rgds.platform` resolves to an OCI-published kro `ResourceGraphDefinition` via `akua.toml`. `akua.helm` / `akua.kustomize` / `akua.rgd` are engine callables shipped with the binary.

### Four sources

- **Helm** — the webapp chart, values mapped from the public schema.
- **Kustomize** — a local overlay under `./overlays/`, building a `ServiceMonitor` + a patched ConfigMap.
- **kro RGD** — the platform glue (e.g. app-scoped DNS record + cert) instantiated with the current App's metadata. `kro.rgd(...)` produces the ResourceGraphDefinition as a regular K8s manifest; kro's controller picks it up from the rendered YAML.
- **Inline KCL** — a `NetworkPolicy` authored directly in KCL. No external engine needed.

### Aggregation

```python
_app     = helm.template(helm.Template { chart = webapp.Chart, values = ... })
_monitor = kustomize.build(kustomize.Build { path = "./overlays" })
_netpol  = NetworkPolicy { ... }
_glue    = kro.rgd(kro.Rgd { definition = platform.RGD, instance = { ... } })

resources = [*_app, *_monitor, _netpol, _glue]
```

One Package, one flat resource list, one render output. ArgoCD/Flux
applies the rendered YAML as it would any other manifest set; kro's
controller sees the RGD and reconciles its instances.

## Render

```sh
akua add                         # resolve deps
akua render --inputs inputs.yaml --out ./deploy
```

Result:

```
deploy/
├── 000-deployment-webapp.yaml          # from helm
├── 001-service-webapp.yaml             # from helm
├── 002-ingress-webapp.yaml             # from helm
├── 003-configmap-webapp.yaml           # from kustomize
├── 004-servicemonitor-webapp.yaml      # from kustomize
├── 005-networkpolicy-webapp.yaml       # from inline KCL
└── 006-resourcegraphdefinition-glue.yaml  # from kro.rgd — kro reconciles it
```

## See also

- [package-format.md](../../docs/package-format.md) — the Package spec
- [embedded-engines.md](../../docs/embedded-engines.md) — which engines are embedded; escape-hatch via `--engine=shell`
- [architecture.md#what-akua-is-not](../../docs/architecture.md) — why we compose reconcilers instead of replacing them
