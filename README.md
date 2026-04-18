# Akua

> **Cloud-native package build, transform, and preview toolkit.**
>
> Author a package with Helm / KCL / helmfile sources + a JSON Schema of
> customer-configurable inputs. Akua produces a deployable, OCI-addressable
> Helm chart — with rendered manifests verifiable in Rust, Node, or a browser
> via the same WASM core.

> ⚠️ **Status: Pre-alpha.** Nothing here is stable. APIs, schemas, and even the project name are subject to change. Do not build production workloads on this yet.

---

## What is Akua?

Akua is the **authoring and build pipeline** between your cloud-native
sources and a deployable OCI artifact. It assembles multiple sources into
an umbrella Helm chart, runs CEL-based transforms on customer inputs,
embeds full Helm v4 template rendering (no `helm` CLI required), and pushes
the result to any OCI registry.

```
┌────────┐       ┌───────┐       ┌────────────────────┐       ┌─────────┐
│  Akua  │  ──▶  │  OCI  │  ──▶  │  ArgoCD / Flux /   │  ──▶  │ Cluster │
│ (build)│       │artifact│      │  Helm / whatever   │       │         │
└────────┘       └───────┘       │  (deploy + sync)   │       └─────────┘
                                  └────────────────────┘

     ▲                                ▲                             ▲
     │                                │                             │
  Akua's scope              Someone else's job               Someone else's job
```

Akua's scope **ends** at "produce an OCI-addressable chart." ArgoCD, Flux,
`helm install`, or `kubectl apply` picks up from there.

## Single binary, zero external CLI deps

In the default flow, `akua` needs nothing on your `$PATH` — not even `helm`.
The Helm v4 template engine is embedded as a wasip1 reactor module hosted
via wasmtime; OCI + HTTP chart dep fetching is native Rust.

| Subcommand | External dep | How |
|---|---|---|
| `akua build` | None | Pure Rust umbrella assembly + CEL transforms |
| `akua preview` / `tree` / `lint` / `inspect` / `attest` | None | In-process |
| `akua publish` | None | `oci-client` (pure Rust, Helm v4–compatible media types + annotations) |
| `akua render --engine helm-wasm` (default) | None | Embedded Helm v4 engine + native fetch |
| `akua render --engine helm-cli` | `helm` CLI | Legacy path, retained for compat |

## What Akua is not

Akua is **not a deployment tool**. It doesn't apply manifests to a cluster,
watch for drift, or orchestrate rollbacks. Those concerns belong to:

- [**ArgoCD**](https://argo-cd.readthedocs.io/) or [**Flux**](https://fluxcd.io/) — GitOps continuous delivery
- [**Helm**](https://helm.sh/) via `helm install` — direct installs
- `kubectl apply` — imperative deployment

Akua produces artifacts those tools consume.

Closest tools that overlap with **Akua's build scope** (not ArgoCD's deploy scope):

| Tool | Overlap | Key difference |
|---|---|---|
| [Porter](https://getporter.org/) | Cloud-native app bundles | CNAB format, not OCI-native |
| [werf](https://github.com/werf/werf) | Build + deploy | werf does deploy too; Akua stays build-only |
| [Timoni](https://timoni.sh/) | CUE-based Helm alternative | Replaces Helm; Akua wraps Helm |
| [Carvel ytt/kbld](https://carvel.dev/) | YAML templating | Narrower — only YAML/image resolution |
| [Helmfile](https://github.com/helmfile/helmfile) | Multi-release orchestration | Different direction (many releases vs. one composed package). Akua supports helmfile as a *source engine* — see `examples/helmfile-package/`. |

## Why does this exist?

Kubernetes packaging is painful because there's no standard way to build an
artifact that:

1. Composes multiple sources (Helm chart + KCL-authored component + upstream chart)
2. Declares which values are customer-configurable at install time
3. Runs custom transformation logic via CEL (cross-field references, slugify, hostname templates)
4. Previews the resolved result live in the browser **before** anything deploys
5. Produces a reproducible, content-addressed OCI artifact

Helm gets close but is chart-only. GitOps tools (Argo, Flux) deploy
artifacts but don't author them. Akua fills the gap between "I have
sources + a schema + transforms" and "I have a deployable Helm chart
ready for any OCI-aware deployer."

## Where this is going

Akua today ships Gen 3: Helm charts on OCI. The ambition is **Gen 4** —
shipping the renderer alongside the sources so any deployer that speaks
WASM can consume any package, regardless of which engine (helm / kcl /
helmfile / kustomize / future) the author used. See
[`docs/vision.md`](docs/vision.md) for the full four-generations framing,
the bundle format sketch (multi-layer OCI with engine layer dedup), and
the adoption path. Not in v1; it's what v2+ is for.

## Relationship to [CNAP](https://github.com/cnap-tech)

Akua is the **open-source build layer** of CNAP's package platform. CNAP's
hosted product (marketplace, billing, tenancy, managed deploy) is built on
top of Akua.

| Open Source (Akua) | Proprietary (CNAP-hosted) |
|---|---|
| Rust core: source fetch, umbrella charts, CEL transforms, Helm render, OCI push | Marketplace listings, pricing, subscriptions, tenancy |
| CLI: `akua build`, `akua preview`, `akua render`, `akua publish`, … | Customer install workflow (wraps ArgoCD for deploy) |
| WASM bindings (`@akua/core-wasm`) for browser preview | Cloud-hosted Package Studio (collaborative IDE, workspaces) |
| Engine plugins: helm / kcl / helmfile | Revenue sharing, customer install tracking |
| Embedded Helm v4 template engine (Go→wasip1 via wasmtime) | Compliance certifications (SOC 2, HIPAA infra) |
| Native OCI publish + fetch (oci-client) | CNAP-hosted OCI registry for managed builds |

**Deployment uses ArgoCD** (open-source, not proprietary to CNAP) — Akua
chart digests feed into ArgoCD `Application` resources that sync to
customer clusters.

Use Akua standalone against your own clusters for free. Or use CNAP's
hosted platform for managed end-to-end. Both paths consume the same
Akua core.

## Using Akua

### CLI

```bash
# From a package dir containing package.yaml + values.schema.json + sources:
akua lint                           # validate schema
akua tree                           # show umbrella dependency structure
akua preview --inputs '{"httpRoute.hostname":"acme"}'
                                    # resolve inputs, print values.yaml

akua build --out dist/chart         # write Chart.yaml + values.yaml + .akua/metadata.yaml
akua render --out dist/chart --inputs '{...}'
                                    # embedded Helm engine → rendered Kubernetes YAML
akua publish --chart dist/chart --to oci://ghcr.io/you/my-pkg
                                    # native OCI push (no helm CLI)

akua attest --chart dist/chart      # SLSA v1 provenance predicate for cosign
akua inspect --chart dist/chart     # show .akua/metadata.yaml provenance

akua diff oci://ghcr.io/you/chart:1.0.0 oci://ghcr.io/you/chart:1.1.0
                                    # structural diff — metadata, deps,
                                    # values defaults, schema fields
```

### TypeScript SDK ([@akua/sdk](https://jsr.io/@akua/sdk))

```typescript
import { init, pullChart, inspectChartBytes, buildUmbrellaChart, packChart } from '@akua/sdk';

await init();

// Pull a chart — dispatches on scheme (oci:// or https://).
const bytes = await pullChart('oci://ghcr.io/stefanprodan/charts/podinfo:6.7.1');
const info = await inspectChartBytes(bytes);

// Compose an umbrella chart + pack a deployable .tgz, no helm CLI.
const umbrella = buildUmbrellaChart('my-pkg', '0.1.0', sources);
const tgz = await packChart(umbrella, subcharts, { metadata: buildMetadata(sources) });
```

Same Rust core powers native CLI + browser live-preview — no duplicate TS
implementation, no drift. Node entry (`@akua/sdk`) ships chart pull,
inspect, umbrella assembly, pack, and `dockerConfigAuth`; browser entry
(`@akua/sdk/browser`) is the same minus Node-only helpers. KCL rendering
in the browser uses upstream's
[`@kcl-lang/wasm-lib`](https://www.npmjs.com/package/@kcl-lang/wasm-lib) +
JS glue; akua-wasm does not compile a KCL engine.

AI coding agents use `akua` via the terminal directly — every command takes
JSON inputs and emits structured output; no separate MCP server layer.

## Project structure

Rust workspace + TypeScript packages:

```
akua/
├── crates/
│   ├── akua-core/              # Pipeline: sources, schema, CEL, umbrella,
│   │                           #   render, publish, attest, metadata, fetch
│   ├── akua-cli/               # The `akua` binary
│   ├── akua-wasm/              # wasm-pack bindings for browser/Node
│   └── helm-engine-wasm/       # Embedded Helm v4 template engine
│                               #   (Go→wasip1, hosted via wasmtime)
├── packages/
│   ├── core-wasm/              # @akua/core-wasm (raw wasm-pack output; internal)
│   └── sdk/                    # @akua/sdk — public TypeScript SDK (published to JSR)
├── examples/
│   ├── hello-package/          # Minimal Helm chart + CEL hostname transform
│   ├── kcl-package/            # KCL-authored component
│   └── helmfile-package/       # helmfile-wrapped release
├── schemas/                    # package.yaml + values-input JSON Schemas
└── docs/
    ├── architecture.md
    ├── design-notes.md         # The WHY — positioning, invariants, trade-offs
    ├── use-cases.md            # The HOW — author → install → deploy flows
    ├── roadmap.md
    └── getting-started.md
```

## Roadmap summary

See [`docs/roadmap.md`](docs/roadmap.md) for the full status. Landed:

- ✅ Pure-algorithm core (hash, source, values, schema, metadata, attest)
- ✅ Umbrella chart assembly; `akua build` / `tree` / `preview` / `lint`
- ✅ CEL expressions (`x-input.cel`) with cross-field references
- ✅ WASM bindings for browser + Node
- ✅ [`@akua/sdk`](https://jsr.io/@akua/sdk) on JSR — pullChart (OCI + HTTP Helm), inspectChartBytes, buildUmbrellaChart, packChart, buildMetadata, dockerConfigAuth
- ✅ Engine plugins: helm, kcl (native Rust), helmfile (CLI shim)
- ✅ Native OCI publish + fetch (oci-client + raw reqwest streaming)
- ✅ SLSA v1 provenance via `akua attest` + `.akua/metadata.yaml` sidecar
- ✅ Embedded Helm v4 template engine + native chart-dep fetcher — **zero external CLI deps by default**
- ✅ Library-safe (no process CWD mutation; CNAP backend can embed akua-core in a multi-threaded server)

Upcoming: install UI reference (React + rjsf + `@akua/sdk/browser`),
Package Studio IDE, HIP proposals upstream.

## Contributing

Pre-alpha. Issues and discussions welcome. PRs against a churning API
create friction, but small focused fixes (typos, doc clarity, test
coverage) are always appreciated. See [CONTRIBUTING.md](./CONTRIBUTING.md).

## Relationship to the cloud-native ecosystem

- **[Helm](https://helm.sh/)** — Akua produces vanilla Helm charts; we wrap Helm, don't replace it. Helm v4's `pkg/engine` is the template engine Akua embeds.
- **[Helm 4 plugins (HIP-0026)](https://helm.sh/community/hips/hip-0026/)** — Relevant to our roadmap (Extism-based plugin host) but not wired yet. Current engines use direct embedding.
- **[OCI](https://opencontainers.org/)** — Akua charts are OCI-addressable with media types matching Helm v4 conventions. Any OCI-aware deployer consumes them.
- **[KCL](https://kcl-lang.io/)** — First-class source-engine alternative. Native Rust linkage via upstream's `kcl-lang` crate.
- **[Helmfile](https://github.com/helmfile/helmfile)** — Supported as a source engine (`engine: helmfile`) for users migrating existing multi-release configs. Shells to `helmfile` at build time.
- **[wasmtime](https://wasmtime.dev/)** — Runtime hosting the embedded Helm template engine.

## Naming

"Akua" — Hawaiian for "divine spirit" — echoes **aqua**, water. That fits
the cloud-native tradition: Docker loads the cargo, **Helm** steers the
ship, **Harbor** stores what's shipped, **Kubernetes** (Greek:
*kubernḗtēs*, "helmsman") pilots the fleet. Akua is the current
underneath — the flow that carries your sources, transforms them in
motion, and delivers a sealed package to the harbor.

Water fits the job functionally too:

- **Flows through channels** — sources into an umbrella, values into manifests, bytes into an OCI registry
- **Takes the shape of its container** — any source format, any target runtime, same pipeline
- **Transparent** — see through it at every stage (live preview, reproducible builds)
- **Carries things between ports** — between local, CI, and managed infrastructure

The name is provisional while the project is in pre-alpha.

## License

Apache License 2.0 — see [LICENSE](./LICENSE).

## Acknowledgments

- [Helm community](https://helm.sh/community/) for the plugin architecture proposal (HIP-0026) we're tracking
- [KCL authors](https://github.com/kcl-lang) for a clean Rust-native embedding story
- [Extism](https://extism.org/) — not currently used but a likely future layer for third-party plugins
- [wasmtime](https://wasmtime.dev/) for the runtime that makes Go→wasip1 embedding practical
