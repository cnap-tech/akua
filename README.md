<p align="center">
  <a href="https://akua.dev"><img src="docs/mascot.png" alt="akua mascot" height="170"></a>
</p>
<h1 align="center">akua</h1>

<p align="center">
  <a href="https://github.com/cnap-tech/akua/actions/workflows/cli-release.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/cnap-tech/akua/cli-release.yml?branch=main&label=CI&style=flat-square"></a>
  <a href="https://github.com/cnap-tech/akua/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/cnap-tech/akua?label=release&style=flat-square"></a>
  <a href="https://jsr.io/@akua/sdk"><img alt="JSR" src="https://jsr.io/badges/@akua/sdk?style=flat-square"></a>
  <a href="./LICENSE"><img alt="License" src="https://img.shields.io/badge/license-Apache--2.0-blue?style=flat-square"></a>
  <a href="#status"><img alt="Status" src="https://img.shields.io/badge/status-pre--alpha-orange?style=flat-square"></a>
</p>

<p align="center">
  <a href="./docs/getting-started.md">Documentation</a>
  &nbsp;•&nbsp;
  <a href="https://akua.dev">Playground</a>
  &nbsp;•&nbsp;
  <a href="https://jsr.io/@akua/sdk">SDK</a>
  &nbsp;•&nbsp;
  <a href="https://github.com/cnap-tech/akua/issues">Issues</a>
</p>

### [Read the docs →](./docs/getting-started.md)

## What is akua?

akua is an all-in-one toolkit for cloud-native packaging. It ships as a single executable called `akua`, written in Rust.

At its core, packages are authored in **KCL** — a typed configuration language with real types, functions, and imports. Existing Helm charts, kro RGDs, and Kustomize bases are callable KCL functions, so the whole ecosystem works unchanged.

```bash
akua init my-app                      # scaffold a typed package
akua render --inputs inputs.yaml      # render to raw YAML, signed
```

The `akua` command-line tool is also a package manager, deploy driver, dev loop, policy engine, and audit spine. Instead of juggling `helm` + `crane` + `cosign` + `kubectl` + a CORS-proxy backend, you only need `akua`.

```bash
akua diff v1 v2                       # structural diff (schema, sources, manifests)
akua publish oci://pkg.example.com/my-app:v1   # signed + SLSA L3 attested
akua deploy --to=argo                 # hydrate a PR against the deploy repo
akua dev                              # sub-second hot-reload against a local cluster
akua inspect oci://...                # audit any published package, no install
```

Designed agent-first: auto-detects Claude Code, Cursor, Codex, Gemini CLI, Goose, Amp, OpenCode, and Cline, emitting structured JSON. Ships a [skills library](skills/) conforming to the [Agent Skills Specification](https://agentskills.io).

<p align="center">
  <img alt="akua init → akua render → akua diff"
       src="docs/hero.gif"
       width="840">
</p>

## Install

```sh
# with install script (recommended)
curl -fsSL https://akua.dev/install | sh

# with Homebrew
brew install cnap-tech/tap/akua

# from source
cargo install --git https://github.com/cnap-tech/akua akua-cli

# into your AI agent (universal)
npx skills install github:cnap-tech/akua/skills
```

Prebuilt binaries live on [GitHub Releases][releases], each with a SHA-256 checksum. Agent setup for Claude Code, Cursor, Codex, Gemini CLI, Goose, Amp, and 25+ others: [`docs/agent-usage.md`](docs/agent-usage.md).

> [!WARNING]
> **Pre-alpha.** APIs, CLI flags, and the `v1alpha1` schema shape are subject to change. Don't build production workloads on this yet. Do file issues if something surprises you.

## Quick start

Scaffold a new package:

```sh
akua init my-app && cd my-app
```

Add a chart — generates a typed KCL subpackage with autocomplete + validation:

```sh
akua add chart oci://ghcr.io/cloudnative-pg/charts/cluster --version 0.20.0
```

Edit `package.k` in your editor with full LSP support. Validate:

```sh
akua lint
```

Render against inputs — produces committable raw YAML:

```sh
akua render --inputs inputs.yaml --out ./deploy
```

Structural diff against a published version:

```sh
akua diff oci://pkg.akua.dev/my-app:0.1.0 oci://pkg.akua.dev/my-app:0.2.0
```

Publish, signed and attested:

```sh
akua publish --to oci://ghcr.io/you/my-app --tag v0.2.0
```

Hot-reload development against a local cluster:

```sh
akua dev
# watching ./ for changes · target: local (kind cluster) · ui: http://localhost:5173
```

Full CLI reference: [`docs/cli.md`](docs/cli.md). Universal contract (the invariants every verb honors): [`docs/cli-contract.md`](docs/cli-contract.md). Runnable examples: [`docs/examples/`](docs/examples/).

## TypeScript SDK

`@akua/sdk` mirrors the CLI programmatically. Same Rust pipeline, compiled to WASM, published to [JSR]. Works in Node, Bun, Deno, and modern browsers.

```ts
import { Akua } from '@akua/sdk';

const akua = new Akua({ registry: 'oci://pkg.akua.dev' });

// Audit any published package
const pkg = await akua.inspect('oci://pkg.akua.dev/webapp-postgres:1.0');

// Render with inputs
const result = await akua.render({
  path: './my-pkg',
  inputs: { appName: 'checkout', hostname: 'checkout.example.com', replicas: 5 }
});

// Structural diff
const diff = await akua.diff('v1.0', 'v1.1');

// Deploy + wait
const handle = await akua.deploy({ app: 'checkout', to: 'argo' });
await handle.waitReady({ timeout: '5m' });
```

The browser entry point (`@akua/sdk/browser`) exposes the read-only subset — inspect, diff, render, verify. No backend. No cluster. Powers the playground at [akua.dev](https://akua.dev).

Full SDK reference: [`docs/sdk.md`](docs/sdk.md).

## Architecture

Three surfaces, one core:

- **[`akua-core`](crates/akua-core)** — the Rust pipeline: KCL interpreter (via `kclvm-rs`), source resolution, OCI + HTTP fetch, render, policy, attest, diff. Deterministic; sandboxed; content-addressable cache.
- **[`akua-cli`](crates/akua-cli)** — the `akua` binary. Twenty verbs, one mental model. Every verb JSON-first, idempotent, typed exit codes — see [`docs/cli-contract.md`](docs/cli-contract.md).
- **[`@akua/sdk`](packages/sdk)** — TypeScript bindings over the same core. Node + browser entries. See [`docs/sdk.md`](docs/sdk.md).

KCL is the authoring language; Helm, kro RGDs, kustomize are callable KCL functions (`helm.template(...)`, `rgd.instantiate(...)`, `kustomize.build(...)`). The whole ecosystem is consumable unchanged — no chart forks to rename values.

Deep dives:
[`docs/architecture.md`](docs/architecture.md) ·
[`docs/cli.md`](docs/cli.md) ·
[`docs/cli-contract.md`](docs/cli-contract.md) ·
[`docs/sdk.md`](docs/sdk.md) ·
[`docs/agent-usage.md`](docs/agent-usage.md) ·
[`skills/`](skills/) ·
[`docs/examples/`](docs/examples/) ·
[`docs/vision.md`](docs/vision.md) ·
[`docs/roadmap.md`](docs/roadmap.md)

## Scope

|   | akua |
|---|---|
| **Does** | Type-safe KCL packaging · Consume existing Helm/kro/kustomize sources as callable functions · Render at CI to committable raw YAML · Structural diff · SLSA v1 attestation · Signed OCI publish · Sub-second dev hot-reload · Agent-friendly CLI contract |
| **Doesn't** | Apply manifests to a cluster (that's ArgoCD/Flux/kro/kubectl) · Manage cluster-side RBAC · Replace Helm's template engine (we embed it) · Sign container images (that's `cosign`; `akua attest` emits the predicate) · Curate a package catalog (we provide the substrate; the ecosystem publishes) |

Interoperates with:
[**ArgoCD**](https://argo-cd.readthedocs.io/) / [**Flux**](https://fluxcd.io/) — render-at-CI; reconcile the raw YAML ·
[**kro**](https://github.com/kubernetes-sigs/kro) — emit an RGD for runtime late-binding when needed ·
[**Helm**](https://helm.sh/) — `helm.template(chart)` consumes any existing chart unchanged ·
[**Crossplane**](https://www.crossplane.io/) — emit XR Compositions for multi-cloud infra ·
[**kubectl**](https://kubectl.docs.kubernetes.io/) — bare apply.

Comparison matrix: [`docs/design-notes.md`](docs/design-notes.md).

## Status

What's shipped:

- `@akua/sdk` 0.4 on JSR — `pullChart` (OCI + HTTP), `packChart`,
  `inspectChartBytes`, `dockerConfigAuth`, streaming, cache, SSRF guard.
- CLI 0.1.0 on GitHub Releases — 4 platforms (x86_64/aarch64 ×
  linux/darwin), Windows x86_64 coming in 0.1.1.
- Embedded Helm v4 template engine (Go → wasip1) hosted via
  wasmtime. No `helm` binary needed.
- Native OCI publish + HTTP Helm pull (SSRF-guarded, size-capped,
  digest-verified).
- SLSA v1 provenance via `akua attest`; `.akua/metadata.yaml` sidecar.
- `akua diff` — structural chart comparison (metadata · deps ·
  values defaults · schema input-field deltas).

What's next: full roadmap at
[`docs/roadmap.md`](docs/roadmap.md).

## Security

Security is treated as a first-class product concern, not a
follow-up. See [SECURITY.md](SECURITY.md) for the threat model,
fixed attack surfaces (tar symlinks, SSRF, source-path confinement,
credential redaction, CEL timeouts, LRU cache), and vulnerability
disclosure process.

## OSS and commercial

akua follows the bun/deno pattern — one brand, one binary, free OSS CLI, paid hosted platform for teams who want it.

**Free forever:** the `akua` CLI, every verb. KCL plugin library, browser playground, signing + distribution substrate for anyone's packages, `tier/dev` policy, deploy to any reconciler (ArgoCD, Flux, kro, Helm, kubectl) or third-party substrate (Fly, Cloudflare).

**Commercial (on [akua.dev](https://akua.dev)):** curated signed policy tiers (`tier/startup`, `tier/production`, `tier/soc2`, `tier/hipaa`, `tier/fedramp-moderate`), managed review surface, cross-repo rollout orchestration, hosted git + CI runners, merchant infrastructure for ISVs, learning loop across customers. `akua deploy --to=akua` is the funnel.

OSS escape hatches are real. Every package you publish is git-committed + OCI-distributed; catalog is forkable; you can self-host; you can leave. You won't want to — but the ability to is a feature, not a threat.

## Contributing

Pre-alpha means APIs churn. Small focused fixes (typos, doc
clarity, test coverage, security findings) are always welcome; PRs
against in-flight features may hit merge friction.
See [CONTRIBUTING.md](CONTRIBUTING.md) and
[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## Naming

"Akua" — Hawaiian for *divine spirit*; echoes **aqua**, water. Fits
the cloud-native tradition: Docker loads the cargo, **Helm** steers
the ship, **Harbor** stores what's shipped, **Kubernetes** (Greek
*kubernḗtēs*, "helmsman") pilots the fleet. Akua is the current
underneath — the flow that carries your sources, transforms them in
motion, and delivers a sealed package to the harbor.

## License

[Apache-2.0](LICENSE).

## Acknowledgments

The [Helm community][hip-0026] for the plugin-architecture proposals
we're tracking. The [KCL authors](https://github.com/kcl-lang) for a
clean Rust-native embedding story.
[wasmtime](https://wasmtime.dev/) for making Go→wasip1 embedding
practical. Everyone who reads a pre-alpha README this far.

[@akua/sdk]: https://jsr.io/@akua/sdk
[JSR]: https://jsr.io/@akua/sdk
[releases]: https://github.com/cnap-tech/akua/releases
[playground]: https://akua.dev
[hip-0026]: https://helm.sh/community/hips/hip-0026/
