# Example 07 — package reuse (cross-package composition)

One akua Package composing another. The reuser pins the base Package by OCI digest in `akua.toml`, imports its `Input` schema into its own schema (as a nested field), and renders the base's resources inline alongside its own additions.

This is **what cross-package composition looks like when the full spec lands** — the shape this axis is settling into. Masterplan §18 and [design-notes.md §6](../../docs/design-notes.md) still list this as an open question for the final API surface. Use this example as the north-star shape; expect minor signature tweaks as the spec locks in.

## The pattern

```
┌──────────────────────────────────────────────────────────────┐
│ platform-base                                                 │  (separate repo, OCI-published)
│  - Publishes:     oci://pkg.acme.corp/platform-base:1.0       │
│  - Input schema:  hostname, tls, monitoring_enabled           │
│  - Produces:      Deployment, Service, Ingress, ServiceMonitor│
└────────────────────┬─────────────────────────────────────────┘
                     │ akua.toml dependency
                     ▼
┌──────────────────────────────────────────────────────────────┐
│ 07-package-reuse (this example)                               │
│  - Adds dashboard ConfigMap                                   │
│  - Adds feature-flag env overlay                              │
│  - Adds one more Ingress host                                 │
│  - Re-exports the base's Input schema as a nested field       │
└──────────────────────────────────────────────────────────────┘
```

The reuser doesn't fork the base. Every publish of the base lands as an OCI digest; the reuser bumps its `akua.toml` pin and re-renders.

## Layout

```
07-package-reuse/
├── akua.toml              declares platform-base as an OCI dep
├── akua.lock              digest + cosign signature of the pinned base
├── package.k             the consumer — composes base + adds specifics
├── inputs.yaml           sample inputs for rendering
└── README.md
```

## The mechanism

A Package published to OCI is **itself an engine source** — the mental model parallel to Helm charts, kro RGDs, Kustomize bases. Any Package can be imported and composed:

```python
import akua.pkg                          # the package-as-source engine callable
import packages.platform_base as base    # the pinned base Package

schema Input:
    """Public input. Composes the base's schema under .base."""

    # Nest the base's Input schema. Consumers fill both.
    base: base.Input

    # Local additions on top.
    dashboard_enabled: bool = True

    @ui(order=99, group="Feature flags")
    experiment_X: bool = False

input: Input

# Render the base Package with the nested input slice. Returns resources[].
_base = pkg.render(base.Package, input.base)

# Add local resources.
_dashboard = {
    apiVersion: "v1"
    kind:       "ConfigMap"
    metadata.name: "${input.base.appName}-dashboard"
    data.enabled: input.dashboard_enabled
}

# Aggregate.
resources = [*_base, _dashboard]

outputs = [
    { kind: "RawManifests", target: "./rendered" }
]
```

Three things fall out of this shape:

1. **Type safety.** The base's `Input` is a nested schema; misspelling a field fails at compile time with a line + column pointer. No "I forgot the base needs `hostname`" at render time.
2. **Pinned by digest.** `akua.toml` + `akua.lock` pin the base to a specific OCI digest. Base publishes v1.1 → you don't pick it up until you `akua add` explicitly. No silent drift.
3. **Signed provenance.** `akua verify` on the consumer walks the attestation chain: the consumer's SLSA predicate includes the base's digest, which carries its own SLSA predicate, which carries the base's sources. Auditable back to the original chart authors.

## Running it

```sh
akua add                                 # resolves deps → writes akua.lock
akua render --inputs inputs.yaml         # composes base + local additions
akua inspect oci://pkg.acme.corp/platform-base:1.0   # peek at what we're pinning
```

## When to reuse vs fork

**Reuse** when the base captures a genuine shared convention: your org's production-ready webapp shape, a standard observability stack, a licensed vendor package you subscribe to.

**Fork** when you need base-level invariants that don't exist yet. Forking means copying the base's `Package.k` into your workspace and editing it. You lose the upgrade path; you own the full surface.

**Don't** reuse to avoid learning KCL. If the base's author didn't anticipate your override, reuse leads to a pile of `postRenderer` hacks that are harder to maintain than a fork.

## Open-question addendum

This axis is specced to the shape shown above but the exact signature of `pkg.render(Package, inputs)` vs alternatives (`base.render(input.base)`, auto-unwrapping imports, etc.) may iterate before the spec locks in. See [masterplan §18 open question 6](https://github.com/cnap-tech/cortex/blob/docs/cnap-masterplan/workspaces/robin/akua-masterplan.md) and [design-notes.md §6](../../docs/design-notes.md). Consumers of this example: don't hard-code the exact callable name in skills or training material yet.

## See also

- [package-format.md §2 Imports](../../docs/package-format.md) — where package-vs-chart imports are described
- [lockfile-format.md](../../docs/lockfile-format.md) — how OCI-pinned Packages flow through `akua.toml`
- [06-multi-engine/](../06-multi-engine/) — Helm / Kustomize / kro — the other engine sources; `pkg` is the fourth
- [02-webapp-postgres/](../02-webapp-postgres/) — the shape of the base Package before it was reused here
