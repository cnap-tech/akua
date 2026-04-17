# Akua

> **Cloud-native package build, transform, and preview toolkit.**
>
> Turn any combination of Helm charts, Knative apps, container images, and custom transformation logic into a single deployable artifact — previewed live in the browser, built anywhere (local, CI, or managed), and shipped as an OCI-addressable package.

> ⚠️ **Status: Pre-alpha.** Nothing here is stable. APIs, schemas, and even the project name are subject to change. Do not build production workloads on this yet.

---

## What is Akua?

Akua is the **authoring and build pipeline** that sits between your cloud-native sources (Helm charts, Knative apps, Dockerfiles, raw Kubernetes manifests, WASM transforms) and a deployable OCI artifact. It produces **byte-identical** output whether you run it locally via the CLI, in CI, or delegate to a managed service like CNAP.

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

Akua's scope **ends** at "produce an OCI-addressable artifact." A separate tool (ArgoCD, Flux, Helm CLI, `kubectl apply`, or CNAP's install workflow) picks up from there and actually deploys.

```
Inputs                         Akua pipeline                     Output
──────                         ─────────────                     ──────
Helm chart ──┐
OCI chart ───┤
Git repo ────┤──▶  fetch → schema merge → umbrella gen →
Dockerfile ──┤      transform (Extism WASM) → validate →         oci://registry/pkg@sha256:...
K8s YAML ────┤      render (Helm + Knative) → package → push
Transform ───┘
```

Think of it as **"Docker + Helm, but for cloud-native application packages with customer-configurable inputs and transformation logic."** Docker builds container images; Helm packages charts; Akua packages *everything* (charts, Knative specs, push-deploy apps, transforms) into a single OCI-addressable artifact ready to be deployed by whatever GitOps / sync / install tool you already use.

## What Akua is not

Akua is **not a deployment tool**. It doesn't apply manifests to a cluster, doesn't watch for drift, doesn't handle rollback orchestration, doesn't do GitOps sync. Those concerns belong to tools like:

- [**ArgoCD**](https://argo-cd.readthedocs.io/) or [**Flux**](https://fluxcd.io/) — GitOps continuous delivery
- [**Helm**](https://helm.sh/) itself via `helm install` — direct chart installs
- `kubectl apply` — imperative deployment
- CNAP's own install workflow (wraps ArgoCD under the hood)

Akua produces artifacts those tools consume. They're complementary. A typical production pipeline looks like:

```
# Build (Akua's job)
akua pkg build
akua pkg publish --to oci://ghcr.io/org/my-pkg

# Deploy (ArgoCD's job, configured separately)
argocd app create my-app \
  --repo oci://ghcr.io/org/my-pkg \
  --revision <tag-or-digest> \
  --dest-namespace prod
```

**In CNAP specifically**, ArgoCD remains the deploy engine. Every install creates an ArgoCD `Application` resource that syncs the Akua-built package to the customer's cluster. Akua replaces the private chart-generation service on the build side; everything downstream (Argo sync, drift detection, rollback) is unchanged.

Closest tools that overlap with **Akua's build scope** (not ArgoCD's deploy scope):

| Tool | Overlap with Akua | Key difference |
|---|---|---|
| [Porter](https://getporter.org/) | Cloud-native app bundles | Uses CNAB format, not OCI-native; no Helm 4 plugin alignment |
| [werf](https://github.com/werf/werf) | Build + deploy | werf does deploy too; Akua stays build-only |
| [Timoni](https://timoni.sh/) | CUE-based Helm alternative | Replaces Helm; Akua wraps Helm |
| [Carvel ytt/kbld](https://carvel.dev/) | YAML templating | Narrower — only YAML/image resolution |
| [Helmfile](https://github.com/helmfile/helmfile) | Multi-release orchestration | Different direction (many releases vs. one composed package) |

## Why does this exist?

Kubernetes packaging is painful because there's no standard way to build an artifact that:

1. Composes multiple sources (e.g., a Knative web app + a Helm-managed Postgres + Redis)
2. Declares which values are customer-configurable at install time
3. Runs custom transformation logic (for per-customer hostnames, computed defaults, multi-field templates)
4. Previews the resolved result live in the browser **before** anything deploys
5. Produces a reproducible, content-addressed OCI artifact

Helm gets close but is chart-only. Knative handles single-image push-deploy but has no composition story. GitOps tools (Argo, Flux) deploy artifacts but don't author them. Akua fills the gap between "I have sources + transforms" and "I have a deployable artifact." **One pipeline, multiple source types, live preview, reproducible output — ready for whatever deploy tool you already use.**

## Relationship to [CNAP](https://github.com/cnap-tech)

Akua is the **open-source build layer** of CNAP's package platform. CNAP's hosted product (marketplace, billing, tenancy, managed deploy infrastructure) is built on top of Akua — Akua handles package authoring and build; CNAP's proprietary layer handles the commercial + operational lifecycle that sits above and below Akua:

| Open Source (Akua) | Proprietary (CNAP-hosted) |
|---|---|
| Rust core: source fetch, umbrella charts, transforms, render | Marketplace listings, pricing, subscriptions, tenancy |
| CLI: `akua build`, `akua preview`, `akua test` | Customer install workflow (wraps ArgoCD for deploy) |
| TS SDK + UI components (editor, form preview, viewer) | Cloud-hosted Package Studio (collaborative IDE, workspaces) |
| MCP server for AI coding agents | Revenue sharing, customer install tracking |
| Extism plugin runtime (WASM) | Compliance certifications (SOC 2, HIPAA infra) |
| OCI push for artifacts | CNAP-hosted OCI registry for managed builds |

**Deployment uses ArgoCD** (open-source, not proprietary to CNAP) — Akua-built packages feed into Argo `Application` resources that sync to customer clusters. CNAP's value on the deploy side is the orchestration around Argo (multi-tenant setup, customer cluster management, Temporal workflows for install lifecycle), not Argo itself.

Use Akua standalone for free against your own clusters (build locally, deploy however you like). Or use CNAP's hosted platform for managed end-to-end: authoring → build → marketplace → customer install. Both paths consume the same OSS Akua core.

The full platform design is captured in **[CEP-0008 (CNAP Enhancement Proposal)](https://github.com/cnap-tech/cnap/blob/main/internal/cep/20260417-chart-transformation-platform.md)** — currently in `cnap-tech/cnap` (private), will be mirrored here as docs solidify.

## Three ways to use Akua

### 1. CLI (local development)

```bash
# Scaffold a new package
akua pkg init

# Add components
akua pkg add helm ./postgres/           # Helm source
akua pkg add app ./web/                 # Knative push-deploy source
akua pkg add transform resolve.ts       # TypeScript transform

# Live preview with test inputs
akua pkg preview --inputs '{"subdomain":"acme"}'

# Run tests
akua pkg test

# Build the OCI artifact
akua pkg build --out ./dist

# Publish to an OCI registry (yours or CNAP-hosted)
akua pkg publish --to oci://ghcr.io/org/my-package

# Deploy is not Akua's job — hand off to ArgoCD/Flux/Helm/kubectl:
#   argocd app create ... --repo oci://ghcr.io/org/my-package
#   flux create helmrelease ... --chart-ref OCIRepository/...
#   helm install ... oci://ghcr.io/org/my-package
```

### 2. TypeScript SDK (in-process)

```typescript
import { Package, buildPackage, preview } from '@akua/sdk';

const pkg = await Package.load('./my-package');
const result = await preview(pkg, { inputs: { subdomain: 'acme' } });

console.log(result.manifests);   // rendered K8s YAML
console.log(result.errors);      // validation issues
```

### 3. MCP server (AI coding agents)

```bash
akua mcp
# Serves MCP tools: pkg.introspect, pkg.preview, pkg.test, pkg.validate, pkg.build
# Point Claude Code, Cursor, or any MCP-compatible agent at it.
```

## Project structure

This is a monorepo combining Rust crates and TypeScript packages:

```
akua/
├── crates/
│   ├── akua-core/       # Rust core: pipeline, fetch, umbrella, transform host
│   ├── akua-cli/        # Rust: the `akua` binary
│   └── akua-wasm/       # WASM bindings for browser consumption
├── packages/
│   ├── core/            # @akua/core — NAPI wrapper around Rust
│   ├── sdk/             # @akua/sdk — high-level TS API
│   ├── ui/              # @akua/ui — Svelte components (editor, form, diff viewer)
│   └── mcp/             # @akua/mcp — Model Context Protocol server
├── examples/
│   ├── hello-package/           # minimal Helm chart + schema
│   ├── hybrid-knative-helm/     # Knative + Helm + cross-component inputs
│   └── transform-examples/      # resolve.* in TS, WASM, Python, KCL
├── schemas/
│   ├── package.schema.json        # top-level package manifest
│   └── values-input.schema.json   # x-user-input extension spec
└── docs/
    ├── architecture.md
    ├── roadmap.md
    └── getting-started.md
```

## Status and roadmap

Akua is being extracted from CNAP's internal chart generation service as it matures. The roadmap (from [CEP-0008](https://github.com/cnap-tech/cnap)):

| Phase | Status | Scope |
|-------|--------|-------|
| v1 — Declarative install fields | Shipped (in CNAP, pre-extraction) | JSON Schema + `x-user-input` + `x-input` for templates/slugify/uniqueness |
| v2 — Naming + template generalization | Near-term | Multi-variable templates, cross-field references, renames |
| v3 — Single-runtime TS escape hatch | Next cycle | `resolve.ts` via V8 isolate, `akua pkg preview` CLI |
| v4 — OSS extraction, Rust core, multi-runtime, OCI | **This repo exists for this work** | `akua-core` in Rust, Extism plugin host, browser execution, local+CI+managed build modes |
| v5 — Package Studio IDE | Multi-quarter | Full in-browser IDE, live reload, test runner, manifest diff |
| v6 — Upstream | Ongoing | Propose values-transform HIP to Helm 4, contribute to Extism JS SDK |

## Contributing

Contribution guidance is placeholder while the project is pre-alpha. Issues and discussions are welcome. Code contributions are on hold until v4 API surface stabilizes — opening PRs against a churning API creates friction for everyone.

See [CONTRIBUTING.md](./CONTRIBUTING.md) for the intended process (filled in as we stabilize).

## Relationship to the cloud-native ecosystem

Akua builds on and contributes to the Kubernetes/Helm ecosystem:

- **[Helm](https://helm.sh/)** — Akua consumes Helm charts as one of several package component types. We don't replace Helm; we wrap it.
- **[Helm 4 plugins (HIP-0026)](https://helm.sh/community/hips/hip-0026/)** — Akua's transform runtime aligns with Helm 4's Extism-based plugin system. Plugins authored for Helm 4 will run in Akua.
- **[Extism](https://github.com/extism/extism)** — Akua uses Extism as the WASM plugin host for transform logic. Any language that compiles to WASM and implements Extism's ABI can author Akua transforms.
- **[Knative](https://knative.dev/)** — Akua supports Knative Service components for push-deploy stateless workloads. Hybrid packages mix Knative and Helm freely.
- **[OCI](https://opencontainers.org/)** — Akua packages are OCI-addressable artifacts. Push to any OCI registry; CNAP or any Helm 4 / OCI-aware deployer consumes them.

## Naming

"Akua" — Hawaiian for "divine spirit" — echoes **aqua**, water. That fits the cloud-native tradition: Docker loads the cargo, **Helm** steers the ship, **Harbor** stores what's shipped, **Kubernetes** (Greek: *kubernḗtēs*, "helmsman") pilots the fleet. Akua is the current underneath — the flow that carries your sources, transforms them in motion, and delivers a sealed package to the harbor.

Water fits the job functionally too:

- **Flows through channels** — sources into an umbrella, values into manifests, bytes into an OCI registry
- **Takes the shape of its container** — any source format, any target runtime, same pipeline
- **Transparent** — you see through it at every stage (live preview, reproducible builds)
- **Carries things between ports** — between your local environment, CI, and managed infrastructure

The name is provisional while the project is in pre-alpha. If a better name emerges before we publish any npm packages or tag a v1 release, we may rename — though we'd want to keep the elemental / nautical feel that puts Akua alongside its cloud-native siblings.

## License

Apache License 2.0 — see [LICENSE](./LICENSE).

## Acknowledgments

- [Helm community](https://helm.sh/community/) for the plugin architecture proposal (HIP-0026) that Akua aligns with
- [Extism](https://extism.org/) for the cross-language WASM plugin runtime
- [Werf/nelm](https://github.com/werf/nelm) for demonstrating richer Helm APIs
- [Shipmight/helm-playground](https://github.com/shipmight/helm-playground) for the in-browser Helm rendering reference
