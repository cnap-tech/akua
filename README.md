# akua

[![CI](https://img.shields.io/github/actions/workflow/status/cnap-tech/akua/cli-release.yml?branch=main&label=CI&style=flat-square)](https://github.com/cnap-tech/akua/actions/workflows/cli-release.yml)
[![Release](https://img.shields.io/github/v/release/cnap-tech/akua?label=release&style=flat-square)](https://github.com/cnap-tech/akua/releases/latest)
[![JSR](https://jsr.io/badges/@akua/sdk?style=flat-square)](https://jsr.io/@akua/sdk)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue?style=flat-square)](./LICENSE)
[![Status](https://img.shields.io/badge/status-pre--alpha-orange?style=flat-square)](#status)

[Documentation](./docs/getting-started.md) &nbsp;•&nbsp; [Playground](https://akua.dev) &nbsp;•&nbsp; [SDK](https://jsr.io/@akua/sdk) &nbsp;•&nbsp; [Issues](https://github.com/cnap-tech/akua/issues)

### [Read the docs →](./docs/getting-started.md)

## What is akua?

akua is an all-in-one toolkit for cloud-native packaging. It ships as a single executable called `akua`, written in Rust.

At its core is the _akua core_, an audited fetch layer plus a Helm v4 template engine compiled to WebAssembly. It's designed as **a drop-in replacement for `helm` + `crane` + a CORS-proxy backend**, so the same pull-inspect-render-diff pipeline works on your laptop, in Node via `@akua/sdk`, and in any browser tab.

```bash
akua init demo                                           # scaffold a new package
akua inspect --oci oci://ghcr.io/.../podinfo:6.7.1       # pull + inspect any chart — no `helm` on PATH
akua diff oci://.../podinfo:6.6.0 oci://.../podinfo:6.7.1  # structural version diff
```

The `akua` command-line tool also implements a builder, a publisher, a schema validator, and a [browser playground](https://akua.dev). Instead of juggling `helm` + `crane` + `cosign` + a CORS-proxy backend, you only need `akua`. Built-in tooling is significantly faster than shelling out to `helm`, and works without a cluster.

```bash
akua build --out dist/chart               # build an umbrella chart with metadata sidecar
akua attest dist/chart                    # emit a SLSA v1 predicate for cosign
akua render dist/chart --inputs '{...}'   # render Kubernetes manifests (no helm binary)
akua publish oci://ghcr.io/you/pkg:v1     # publish with digest-pinning
```

<p align="center">
  <img alt="akua init → akua inspect --oci → akua diff"
       src="docs/hero.gif"
       width="840">
</p>

akua is backed by [CNAP](https://cnap.tech).

## Install

akua supports Linux (x64 & arm64) and macOS (x64 & Apple Silicon). Windows binaries land with `v0.1.1`.

```sh
# with install script (recommended)
curl -fsSL https://akua.dev/install | sh

# with Homebrew
brew install cnap-tech/tap/akua

# from source (any platform with a Rust toolchain)
cargo install --git https://github.com/cnap-tech/akua akua-cli
```

Prebuilt binaries for every target live on [GitHub Releases][releases] — each artefact ships with a SHA-256 checksum file.

> [!WARNING]
> **Pre-alpha.** APIs, CLI flags, and the `v1alpha1` schema shape are subject to change. Don't build production workloads on this yet. Do file issues if something surprises you.

## Quick start

Scaffold a new package:

```sh
akua init my-pkg && cd my-pkg
```

Lint its schema and preview how customer inputs resolve:

```sh
akua lint && akua preview --inputs '{"httpRoute.hostname":"acme"}'
```

Build the umbrella chart (Chart.yaml + values.yaml + `.akua/metadata.yaml`):

```sh
akua build --out dist/chart
```

Render Kubernetes manifests using the embedded Helm v4 engine — no
`helm` binary involved:

```sh
akua render --out dist/manifests --inputs '{...}'
```

Diff your chart against the previously-published version, so you
know what changed before users do:

```sh
akua diff oci://ghcr.io/you/my-pkg:0.1.0 oci://ghcr.io/you/my-pkg:0.2.0
```

Publish to any OCI registry (native Rust, Helm v4-compatible
media types):

```sh
akua publish --chart dist/chart --to oci://ghcr.io/you/my-pkg
```

## TypeScript SDK

`@akua/sdk` is the same Rust pipeline, compiled to WASM, published
to [JSR]. Works in Node, Bun, Deno, and modern browsers.

```ts
import { init, pullChart, buildUmbrellaChart, packChart, buildMetadata } from '@akua/sdk';
import { pullChartCached } from '@akua/sdk/cache';  // Node-only, shares $XDG_CACHE_HOME with the CLI

await init();

// Pull an existing chart from any OCI or classic Helm HTTP repo.
const bytes = await pullChart('oci://ghcr.io/stefanprodan/charts/podinfo:6.7.1');

// Or compose a new umbrella chart from multiple sources, in-browser.
const umbrella = buildUmbrellaChart('my-pkg', '0.1.0', sources);
const tgz = await packChart(umbrella, subcharts, { metadata: buildMetadata(sources) });
```

SSRF guard, size caps, symlink-reject, LRU cache, streaming pull —
all the CLI's hardening ships under the same API. Full reference:
[packages/sdk/README.md](packages/sdk/README.md).

## Architecture

Three surfaces, one core:

- **[`akua-core`](crates/akua-core)** — the Rust pipeline: source
  resolution, CEL transforms, umbrella assembly, OCI + HTTP fetch,
  render, publish, attest, diff.
- **[`akua-cli`](crates/akua-cli)** — the `akua` binary. Feature-gated
  fetch / publish / helm-cli / helm-wasm engines.
- **[`@akua/sdk`](packages/sdk)** — TypeScript bindings over the same
  core via `wasm-pack`. Node + browser entries.

Deep dives:
[`docs/architecture.md`](docs/architecture.md) · 
[`docs/design-notes.md`](docs/design-notes.md) ·
[`docs/design-package-yaml-v1.md`](docs/design-package-yaml-v1.md) ·
[`docs/vision.md`](docs/vision.md)

## Scope

|   | Akua |
|---|---|
| **Does** | Assemble multi-source packages · Validate `x-user-input` schemas · Apply CEL transforms · Pull + pack + publish OCI charts · Emit SLSA provenance · Structurally diff chart versions |
| **Doesn't** | Apply manifests to a cluster · Watch for drift · Orchestrate rollbacks · Manage cluster-side RBAC · Sign images (that's `cosign`'s job; `akua attest` emits the predicate) |

Closest neighbours:
[**Helm**](https://helm.sh/) (Akua produces vanilla Helm charts — we
wrap Helm, don't replace it) ·
[**Timoni**](https://timoni.sh/) (CUE-based alternative) ·
[**helmfile**](https://github.com/helmfile/helmfile) (multi-release
orchestration; Akua supports it as a source engine) ·
[**ArgoCD**](https://argo-cd.readthedocs.io/) /
[**Flux**](https://fluxcd.io/) (GitOps deploy; consume Akua's output).

Full comparison matrix in
[`docs/comparisons.md`](docs/design-notes.md).

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

## Relationship to CNAP

Akua is the open-source package-build layer of
[CNAP](https://github.com/cnap-tech)'s managed platform. Built here,
used there. Use it standalone against your own clusters for free, or
consume CNAP's hosted build-and-deploy service. Both paths run the
same `akua-core`.

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
