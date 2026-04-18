# hybrid-package

Two sources, two engines, one umbrella chart.

```
sources:
  - name: web           # Helm — pulls bitnami/nginx 18.1.0 from the upstream repo
  - name: site-config   # KCL — compiled locally into a static ConfigMap
```

Run:

```bash
akua tree --package examples/hybrid-package
akua build --package examples/hybrid-package --out ./dist/chart
akua package --chart ./dist/chart --out-dir ./dist
```

The built umbrella's `Chart.yaml` has **two** dependencies — one pointing at the
Bitnami OCI/HTTP repo, one pointing at a `file://` directory that the KCL
engine materialised at build time.

## What this exercises

- Engine dispatch by block presence (`helm:` vs `kcl:`).
- Deterministic aliasing: `web` becomes `nginx-<hash>`, `site-config` stays
  `site-config` (KCL-materialised subcharts own their source name).
- Values merge nesting: `replicaCount: 2` lands at `nginx-<hash>.replicaCount`
  in the merged `values.yaml`.

## What this does *not* exercise (yet)

**Cross-source value refs.** In a future v1alpha2, KCL programs will be able
to read from peer sources via `${sources.web.values.replicaCount}` or a CEL
equivalent; today the two sources are independent. See `docs/design-package-yaml-v1.md`
§ "Deferred to v1alpha2+".

Keeping it deferred on purpose — both Porter's dependency DAG and Timoni's
runtime-lookup got cross-source refs wrong in subtle ways, and we'd rather
ship the mechanism once a concrete use case validates the evaluation semantics.
