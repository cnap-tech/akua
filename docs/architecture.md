# Architecture

Akua is a build + transform pipeline for cloud-native packages. Multiple
sources (Helm charts, KCL-authored components, helmfile releases) compose
into an umbrella Helm chart with CEL-computed customer inputs, rendered
in-process via an embedded Helm v4 template engine, and pushed as an
OCI-addressable artifact.

## Pipeline

```
Inputs                         Pipeline                             Output
──────                         ────────                             ──────

package.yaml ──┐
values.schema ─┤                                                    oci://registry/
 .json         │               1. Load manifest                     pkg@sha256:...
Engine         │               2. Resolve source paths              (vanilla Helm
 sources ──────┼──▶            3. Per-source Engine::prepare        chart — ArgoCD
Customer       │               4. Umbrella Chart.yaml + values      consumes natively)
 inputs ───────┤               5. CEL evaluate → resolved values.yaml
(JSON from the ┤               6. Fetch remote deps (OCI + HTTP, native)
 install form) ┘               7. Render via embedded Helm engine
                               8. Emit .akua/metadata.yaml provenance
                               9. Emit SLSA v1 predicate (on `akua attest`)
                               10. Push to OCI (on `akua publish`)
```

Every stage runs in-process from Rust. No `helm` CLI, no `kcl` CLI (the
KCL engine links the official `kcl-lang` Rust crate), no shell-outs in the
default render flow. `helmfile` engine is the one exception — it inherently
needs the `helmfile` binary because its whole value is orchestrating helm
invocations.

## Three consumers, one core

The Rust core powers three equal consumers:

1. **CLI** — the `akua` binary (also the surface AI coding agents call via their shells)
2. **Browser** — Package Studio IDE, customer install UI (via `@akua/core-wasm` from wasm-pack)
3. **Server embedding** — any backend linking akua-core as a library in build workers (e.g. backend services invoking `@akua/sdk`)

Library consumers call into the same crate as the CLI. No privileged code
path.

## Three build modes

All produce byte-identical OCI artifacts (deterministic engines only — see
[`design-notes.md §10`](./design-notes.md#10-engine-determinism-reality-check)):

1. **Local** — `akua build` / `akua publish` on a developer's machine
2. **CI** — same tool in GitHub Actions / GitLab CI / etc.
3. **Managed** — a platform's orchestration layer invokes the same core server-side, producing byte-identical output

## Component layers

```
┌─────────────────────────────────────────────────────────────┐
│  akua-core  (Rust)                                          │
│                                                             │
│  - hash, source, values, schema, metadata, attest           │
│  - Engine trait + impls: helm / kcl / helmfile              │
│  - umbrella assembly                                        │
│  - render (via helm-engine-wasm OR helm CLI)                │
│  - fetch: native OCI + HTTP chart dep fetcher               │
│  - publish: native OCI push (oci-client)                    │
└──────────────────┬──────────────────────────────────────────┘
                   │
    ┌──────────────┼──────────────────────┬──────────────┐
    │              │                      │              │
 akua-cli      akua-wasm            helm-engine-wasm     │
 (binary)      (browser/Node)       (Go→wasip1 embedded) │
    │              │                      │              │
    ▼              ▼                      └──────┬───────┘
 `akua` cmd     @akua/core-wasm                  │
                                          wasmtime host
                                          inside akua-core
```

### akua-core responsibilities

- **Source model** (`source.rs`): `HelmSource` + alias computation via djb2 hash
- **Values** (`values.rs`): deep-merge + dot-notation + umbrella alias nesting
- **Schema** (`schema.rs`): `x-user-input` field extraction, CEL evaluation, slugify
- **Umbrella** (`umbrella.rs`): multi-engine dependency assembly
- **Engine** (`engine/`): trait + helm (pass-through), kcl (native Rust), helmfile (CLI)
- **Render** (`render.rs`): orchestrates fetch + render via either embedded wasm or helm CLI
- **Fetch** (`fetch.rs`): native chart-dep resolution (oci-client + reqwest)
- **Publish** (`publish.rs`): Helm-v4-compatible OCI push
- **Attest** (`attest.rs`): SLSA v1 provenance JSON
- **Metadata** (`metadata.rs`): `.akua/metadata.yaml` build lineage

### helm-engine-wasm (the embedded renderer)

Separate crate (`crates/helm-engine-wasm/`). Go wrapper around
`helm.sh/helm/v4/pkg/engine.Render` compiled to wasip1 (reactor module
via `-buildmode=c-shared`), embedded into the akua binary via
`include_bytes!`, hosted via wasmtime. See
[`crates/helm-engine-wasm/README.md`](../crates/helm-engine-wasm/README.md)
for the ABI + size-optimization plan.

## Engine plugins

Each source in `package.yaml` declares an `engine:`. Plugins run at
**authoring time** and produce a chart fragment the umbrella composes.

| Engine | Impl | Output kind |
|---|---|---|
| `helm` (default) | Native Rust pass-through — source is already a chart | Umbrella dependency entry |
| `kcl` | Native Rust via `kcl-lang` crate (git dep; 4 ms eval) | LocalChart (materialized `<source-id>/`) |
| `helmfile` | Shells to `helmfile template` | LocalChart (static subchart wrapping helmfile's output) |

Browser consumers that want to render KCL live use upstream's
[`@kcl-lang/wasm-lib`](https://www.npmjs.com/package/@kcl-lang/wasm-lib)
directly and feed the result into `akua-wasm`'s umbrella assembler — no
KCL engine compiled into the browser bindings.

## See also

- [Vision](./vision.md) — Gen 4 thesis (WASM renderer as universal distribution primitive) + bundle format sketch
- [Package format](./package-format.md) — canonical spec for Packages and UI hints via docstrings + `@ui` decorators
- [Use Cases](./use-cases.md) — end-to-end user journeys (author → install → deploy); both shared-chart and per-customer OCI models with ArgoCD YAML
- [Design Notes](./design-notes.md) — positioning, invariants, engine determinism reality check
- [Roadmap](./roadmap.md) — phase-by-phase status
- [Getting Started](./getting-started.md) — build + first package
