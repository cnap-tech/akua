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
├── akua.mod
├── akua.sum
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

`charts.webapp` resolves to an OCI-published Helm chart via `akua.mod`. `rgds.platform` resolves to an OCI-published kro `ResourceGraphDefinition` via `akua.mod`. `akua.helm` / `akua.kustomize` / `akua.rgd` are engine callables shipped with the binary.

### Four sources

- **Helm** — the webapp chart, values mapped from the public schema.
- **Kustomize** — a local overlay under `./overlays/`, building a `ServiceMonitor` + a patched ConfigMap.
- **kro RGD** — the platform glue (e.g. app-scoped DNS record + cert) instantiated with the current App's metadata. Because it genuinely needs runtime status late-binding, this goes to the `runtime` output; kro's controller reconciles it.
- **Inline KCL** — a `NetworkPolicy` authored directly in KCL. No external engine needed.

### Aggregation with per-source routing

```python
_app     = helm.template(webapp.Chart { ... }, output = "static")
_monitor = kustomize.build("./overlays",       output = "static")
_netpol  = NetworkPolicy { ... }                                     # → static by default
_glue    = rgd.instantiate(platform.RGD, { ... }, output = "runtime")

resources = [*_app, *_monitor, _netpol, *_glue]

outputs = [
    { name: "static",  kind: "RawManifests",             target: "./deploy/static" },
    { name: "runtime", kind: "ResourceGraphDefinition",  target: "./deploy/rgd"    },
]
```

One Package, two deploy paths. ArgoCD applies the static output; kro reconciles the runtime output. No conflict — the resources are disjoint.

## Render

```sh
akua add                         # resolve deps
akua render --inputs inputs.yaml # renders both outputs into ./deploy/
```

Result:

```
deploy/
├── static/
│   ├── deployment.yaml          # from helm
│   ├── service.yaml             # from helm
│   ├── ingress.yaml             # from helm
│   ├── configmap.yaml           # from kustomize
│   ├── servicemonitor.yaml      # from kustomize
│   └── networkpolicy.yaml       # from inline KCL
└── rgd/
    └── platform-glue.yaml       # the RGD for kro to reconcile
```

## See also

- [package-format.md §6 Per-source output routing](../../docs/package-format.md) — the spec for named outputs
- [embedded-engines.md](../../docs/embedded-engines.md) — which engines are embedded; escape-hatch via `--engine=shell`
- [architecture.md#what-akua-is-not](../../docs/architecture.md) — why we compose reconcilers instead of replacing them
