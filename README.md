# Akua

> **Cloud-native package build, transform, and preview toolkit.**
>
> Turn any combination of Helm charts, Knative apps, container images, and custom transformation logic into a single deployable artifact — previewed live in the browser, built anywhere (local, CI, or managed), and shipped as an OCI-addressable package.

> ⚠️ **Status: Pre-alpha.** Nothing here is stable. APIs, schemas, and even the project name are subject to change. Do not build production workloads on this yet.

---

## What is Akua?

Akua is the authoring and build pipeline that sits between your cloud-native sources (Helm charts, Knative apps, Dockerfiles, raw Kubernetes manifests, WASM transforms) and a deployable OCI artifact. It produces **byte-identical** output whether you run it locally via the CLI, in CI, or delegate to a managed service like CNAP.

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

Think of it as **"Docker + Helm, but for cloud-native application packages with customer-configurable inputs and transformation logic."**

## Why does this exist?

Kubernetes deployment is painful because there's no standard way to build a package that:

1. Composes multiple sources (e.g., a Knative web app + a Helm-managed Postgres + Redis)
2. Declares which values are customer-configurable at install time
3. Runs custom transformation logic (for per-customer hostnames, computed defaults, multi-field templates)
4. Previews the result live in the browser before deploying
5. Produces a reproducible OCI artifact

Helm gets close but is chart-only. Knative handles single-image push-deploy but has no composition story. CI/CD tools (Argo, Flux) deploy but don't author. Akua fills the gap: **one pipeline, multiple source types, live preview, reproducible output.**

## Relationship to [CNAP](https://github.com/cnap-tech)

Akua is the **open-source core** of CNAP's package platform. CNAP's hosted product (marketplace, billing, tenancy, managed deploy) is built on top of Akua. The split:

| Open Source (Akua) | Proprietary (CNAP-hosted) |
|---|---|
| Rust core: sources, umbrella charts, transforms, render | Marketplace listings, pricing, subscriptions |
| CLI: `akua build`, `akua preview`, `akua test` | Managed deploy to customer clusters |
| TS SDK + UI components (editor, form preview, viewer) | Cloud-hosted Package Studio (collaborative IDE) |
| MCP server for AI coding agents | Customer install tracking, revenue sharing |
| Extism plugin runtime (WASM) | Compliance certifications (SOC 2, HIPAA infra) |

Use Akua standalone for free against your own clusters, or use CNAP's hosted platform for the managed experience. Both consume the same OSS.

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

"Akua" is a Hawaiian word meaning "divine spirit" or "god." It was chosen for its brevity, distinctiveness, and because it's not overloaded in the cloud-native space. We recognize "Akua" sounds similar to "Aqua" (the security company); the projects serve completely different use cases.

The name is provisional while the project is in pre-alpha. If a better name emerges before we publish any npm packages or tag a v1 release, we may rename.

## License

Apache License 2.0 — see [LICENSE](./LICENSE).

## Acknowledgments

- [Helm community](https://helm.sh/community/) for the plugin architecture proposal (HIP-0026) that Akua aligns with
- [Extism](https://extism.org/) for the cross-language WASM plugin runtime
- [Werf/nelm](https://github.com/werf/nelm) for demonstrating richer Helm APIs
- [Shipmight/helm-playground](https://github.com/shipmight/helm-playground) for the in-browser Helm rendering reference
