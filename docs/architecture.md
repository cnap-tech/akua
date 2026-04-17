# Architecture

> Status: draft. The authoritative design doc is currently **CEP-0008** in the private CNAP repo. This doc will be mirrored/expanded here as the project matures.

## Overview

Akua is a build + transform pipeline for cloud-native packages. Inputs (Helm charts, Knative apps, Dockerfiles, raw K8s manifests, transform plugins) flow through seven stages and produce an OCI-addressable artifact.

```
Inputs                       Pipeline                              Output
──────                       ────────                              ──────

Helm chart ────┐
OCI registry ──┤
Git repo ──────┼──▶  1. Source fetch         (auth-pluggable)
Docker image ──┤     2. Schema merge         (values.schema.json + x-user-input)
Source code ───┘     3. Umbrella chart gen   (alias dependencies, nest values)
                     4. Transform plugins    (Extism WASM execution)
                     5. Validation           (schema + transform output + render)
                     6. Package assembly     (tar.gz chart + transforms + metadata)
                     7. OCI push             (immutable content-addressed)  ──▶ oci://registry/pkg@sha256:...
```

## Three consumers, one core

The Rust core powers three equal consumers:

1. **Humans** via Package Studio (CNAP's hosted IDE, uses `@akua/ui`)
2. **AI coding agents** via the MCP server (`@akua/mcp`)
3. **CLI / CI / scripts** via the `akua` binary

Plus a fourth: **CNAP's own workflows** call into the SDK the same way external builders do. No privileged path. Dogfood.

## Three build modes

All produce byte-identical OCI artifacts:

1. **Local** — `akua build` on developer's machine, push to any OCI registry.
2. **CI** — same tool in GitHub Actions / GitLab CI / etc., push to registry, notify CNAP of new revision.
3. **Managed** — CNAP's Temporal workflow runs the same build server-side, pushes to CNAP-hosted registry.

## Component layers

```
                    ┌──────────────────────────┐
                    │  akua-core  (Rust)       │
                    │                          │
                    │  - SourceFetcher trait   │
                    │  - Schema merge          │
                    │  - Umbrella chart gen    │
                    │  - Transform execution   │
                    │  - Helm render            │
                    │  - OCI push              │
                    └──────────┬───────────────┘
                               │
        ┌──────────────────────┼──────────────────────┐
        │                      │                      │
    akua-cli             akua-wasm              (NAPI bindings)
    (binary)             (browser WASM)         (Node.js bindings)
        │                      │                      │
        ▼                      ▼                      ▼
    cnap apps pkg     Browser execution         @akua/core (npm)
    (CLI UX)          (Package Studio preview)        │
                                                       ▼
                                              @akua/sdk / @akua/ui / @akua/mcp
```

## See also

- [Use Cases](./use-cases.md) — end-to-end user journeys (author → install → deploy); both shared-chart and per-customer OCI models with ArgoCD YAML
- [Design Notes](./design-notes.md) — in-repo design rationale (positioning, engine plugins, CEL, provenance, CNAP integration, open questions)
- [Roadmap](./roadmap.md) — phase-by-phase plan
- [Getting Started](./getting-started.md) — using Akua
- [CNAP CEP-0008](https://github.com/cnap-tech/cnap/blob/main/internal/cep/20260417-chart-transformation-platform.md) — upstream design narrative (private)
