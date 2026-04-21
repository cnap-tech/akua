# Architecture

akua is one binary, end-to-end. Every verb is a transformation on typed inputs producing typed, signed, deterministic outputs. No hidden services, no control plane, no required cluster. The CLI does the work; the SDK exposes the same work to library consumers; the browser runs the same work in WebAssembly.

This document describes the **target architecture**. Implementation is tracked in [`roadmap.md`](./roadmap.md) and the masterplan.

## The shape

```
    author                  compile                   consume
    ──────                  ───────                   ───────

    KCL Package      ──▶   akua render    ──▶   reconcilers:
    (*.k + akua.toml)        │                     ArgoCD / Flux / kro
                            │                     Helm release lifecycle
    Rego Policy      ──▶    ├─ embedded           kubectl / Crossplane
    (*.rego)                │  engines:
                            │   KCL
    @ui decorators   ──▶    │   Helm v4
    (on KCL schemas)        │   OPA + Regal
                            │   Kyverno→Rego
    akua.toml + akua.lock ──▶ │   CEL
    (human intent +         │   Kustomize
     digest-pinned ledger)  │   kro (offline)
                            │
                            └─ akua publish  ──▶  OCI registry
                                (signed + SLSA)     (cosign + SLSA v1)
```

Three stages, each independently pluggable. See [`docs/package-format.md`](./package-format.md) and [`docs/policy-format.md`](./policy-format.md) for the authoring shape; [`docs/cli.md`](./cli.md) for every verb; [`docs/sdk.md`](./sdk.md) for library-consumer shape.

## Three consumers, one core

| consumer | surface | transport | when used |
|---|---|---|---|
| **CLI** — `akua` binary | 30 verbs (see [`cli.md`](./cli.md)) | subprocess | developers, CI, agents in sandboxes |
| **SDK** — `@akua/sdk` | same capabilities, Node/Bun-native | **WASM-in-SDK** (v0.1) or **daemon** (`akua serve`, v0.3+) | backend services that embed akua in-process |
| **Browser** — playground at `akua.dev` + local `akua dev` UI | read-only/render subset | WASM module | authoring, review, live-preview |

**Trust contract:** the binary, the SDK, and the browser produce byte-identical output for the same inputs. No "the real thing is behind the paywall." A backend service calling `@akua/sdk.render()` gets the same bytes a developer gets from `akua render` in their terminal.

**SDK transport model:** `@akua/sdk` does not shell out to the `akua` binary. The Rust core is compiled to WASM and bundled inside the npm/JSR package. SDK version = WASM module version; always in lockstep. A long-lived daemon transport (`akua serve`) is on the v0.3 roadmap for high-throughput servers that need warm engine state shared across many parallel callers. See [`docs/sdk.md §Transport model`](./sdk.md#transport-model).

See [`docs/cli-contract.md`](./cli-contract.md) for the universal contract every consumer honors.

## Embedded engines

All engines bundled into the binary via wasmtime (native Rust engines linked directly). `$PATH` is never required. The CLI exposes `--engine=shell` as an escape hatch for developers who need a specific engine version from `$PATH`; the SDK (`@akua/sdk`) never does this — it uses only the embedded WASM core.

See [`docs/embedded-engines.md`](./embedded-engines.md) for the embedding contract, version pinning, and size budget.

## Canonical form is typed code

- **Packages** — authored in **KCL** (`Package.k` with four regions: imports / schema / body / outputs). Published as signed OCI artifacts. See [`docs/package-format.md`](./package-format.md).
- **Policies** — authored in **Rego**. Kyverno / CEL / foreign Rego modules are consumed as compile-resolved imports via `akua.toml`, not runtime string lookups.
- **Higher-level workspace concepts** (App, Environment, Cluster, Secret, Gateway, Workspace, PolicySet, …) — **user-defined KCL schemas** in the consumer's own workspace, shaped to their deployment reality. akua does not ship a KRM vocabulary. Reconcilers (ArgoCD / Flux / kro) consume the raw-Kubernetes output of `akua render`; they don't need akua-specific kinds.

## Determinism

Same inputs + same `akua.lock` + same akua version → byte-identical output. No `now()`, no `random()`, no env reads, no filesystem reads, no cluster reads inside the render pipeline.

See [`design-notes.md §engine-determinism`](./design-notes.md#10-engine-determinism-reality-check) for the pragmatic trade-offs (why Helm stays non-pure even though pure-functional would be cleaner).

## Signing + attestation by default

`akua publish` emits a cosign signature plus a SLSA v1 predicate unless the caller explicitly opts out. Consumers verify by default on pull. See [`docs/lockfile-format.md`](./lockfile-format.md) and [`docs/cli.md`](./cli.md) `publish` / `verify`.

## What akua is not

- **Not a reconciler.** ArgoCD, Flux, kro, kubectl own the cluster side.
- **Not a Kubernetes control plane.** No controllers, no CRDs of our own running against customer clusters.
- **Not a non-Kubernetes deploy target.** We emit formats Kubernetes-ecosystem reconcilers consume. Fly Machines / Cloudflare Workers / AWS Lambda are out of scope.
- **Not a curated package catalog.** Upstream projects publish their own signed packages; we ship the substrate.

## See also

- [CLI reference](./cli.md) · [CLI contract](./cli-contract.md) · [SDK](./sdk.md)
- [Package format](./package-format.md) · [Policy format](./policy-format.md)
- [Lockfile format](./lockfile-format.md) · [Embedded engines](./embedded-engines.md)
- [Agent usage](./agent-usage.md) · [Design notes](./design-notes.md) · [Roadmap](./roadmap.md)
