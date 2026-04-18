<h1 align="center">Akua</h1>

<p align="center">
  <em>The only toolkit that pulls, inspects, diffs, and publishes
  Helm charts from a browser tab —
  <br>no <code>helm</code> CLI, no backend, no cluster.</em>
</p>

<p align="center">
  <a href="https://github.com/cnap-tech/akua/actions/workflows/cli-release.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/cnap-tech/akua/cli-release.yml?branch=main&label=CI&style=flat-square"></a>
  <a href="https://jsr.io/@akua/sdk"><img alt="JSR" src="https://jsr.io/badges/@akua/sdk?style=flat-square"></a>
  <a href="https://github.com/cnap-tech/akua/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/cnap-tech/akua?label=release&style=flat-square"></a>
  <a href="./LICENSE"><img alt="License" src="https://img.shields.io/badge/license-Apache--2.0-blue?style=flat-square"></a>
  <a href="./SECURITY.md"><img alt="Security" src="https://img.shields.io/badge/security-audited-brightgreen?style=flat-square"></a>
  <a href="#status"><img alt="Status" src="https://img.shields.io/badge/status-pre--alpha-orange?style=flat-square"></a>
</p>

<p align="center">
  <a href="https://cnap-tech.github.io/akua/"><strong>▸ Try it in your browser</strong></a> ·
  <a href="./docs/getting-started.md">Getting started</a> ·
  <a href="./docs/architecture.md">Architecture</a> ·
  <a href="./docs/roadmap.md">Roadmap</a> ·
  <a href="https://jsr.io/@akua/sdk">SDK</a>
</p>

<p align="center">
  <img alt="akua init → akua inspect --oci → akua diff"
       src="docs/hero.gif"
       width="840">
</p>
<p align="center">
  <em>Scaffold a package, pull any OCI chart without <code>helm</code>,
  diff two versions structurally — three commands, no backend,
  no cluster.</em>
</p>

> [!WARNING]
> **Pre-alpha.** APIs, CLI flags, and even the `v1alpha1` schema
> shape are subject to change. Don't build production workloads on
> this yet. Do file issues if something surprises you.

## Why

Kubernetes packaging is painful when you want to do more than ship a
single Helm chart. Akua handles the layer above:

- **Embeddable.** Same Rust core runs in the `akua` CLI, in Node
  via [@akua/sdk], and in the browser via WASM. One set of
  algorithms, three surfaces, no drift.
- **CLI-free by default.** `akua build` / `render` / `publish` do
  their own OCI + HTTP Helm dep fetching and template rendering —
  no `helm` on `$PATH`. The `helm-cli` engine is available as a
  legacy fallback for trusted inputs.
- **Verifiable.** Every built chart ships a `.akua/metadata.yaml`
  provenance sidecar. `akua attest` emits a SLSA v1 predicate for
  `cosign`. `akua diff` structurally compares two chart versions —
  metadata, deps, values defaults, schema input-field deltas — so
  you can see what changed before an upgrade.

### Three things nobody else does

<details><summary><b>Pull an OCI/HTTP chart from a browser tab.</b> No backend, no proxy.</summary>
<br>

`@akua/sdk/browser` reimplements the OCI bearer-token dance + HTTP
Helm `index.yaml` lookup + tar-gz unpack + schema merge — all on
top of the browser's native `fetch()` + `DecompressionStream`. Open
[the playground][playground], paste
`https://charts.jetstack.io/cert-manager:v1.16.1`, click Pull: your
browser talks directly to Jetstack. DevTools Network tab confirms.

Docker can't do this. `helm` can't do this. Traditional approach:
stand up a CORS-proxying backend. Akua: ship the Rust core as WASM
and reimplement OCI in JS.

Constraint: needs CORS-friendly registries. Jetstack / Grafana /
JFrog public: works. `oci://ghcr.io` / `oci://docker.io`: blocked
by the registry, not by akua.

</details>

<details><summary><b>Structurally diff two chart versions.</b> Not rendered YAML — the <em>shape</em> of what customers will be asked to configure differently.</summary>
<br>

`helm diff` renders templates (needs values + a cluster). `akua
diff oci://A oci://B` reports Chart.yaml metadata shifts,
dependency adds/removes/updates, `values.yaml` default-value
deltas, `values.schema.json` input-field changes
(required↔optional, type, default, `x-input` CEL transform
rewrites).

3am-pager tool. Non-zero exit on delta → gate CI on "no surprise
upgrades."

</details>

<details><summary><b>Render Helm templates without a <code>helm</code> binary.</b></summary>
<br>

The Helm v4 template engine (Go), compiled to
[wasip1](https://github.com/WebAssembly/WASI),
hosted via [wasmtime](https://wasmtime.dev/) inside `akua-core`.
`akua render` takes a chart, runs the Go template pipeline through
a WASM runtime, returns rendered Kubernetes manifests. No external
`helm` binary anywhere in the pipeline.

|                | binaries needed on PATH | behaviour |
|---|---|---|
| `helm dep update && helm template` | `helm` (+ its config) | uses Helm's own registry auth, cache, redirect rules |
| `akua build && akua render` | *(none)* | uses akua's audited fetch (SSRF guard, size caps, digest verify) |

Most Rust CLIs in Helm's orbit shell out to `helm`. Akua embeds it.

</details>

## Install

```sh
# macOS / Linux — Homebrew tap
brew install cnap-tech/tap/akua

# macOS / Linux — one-liner
curl -fsSL https://raw.githubusercontent.com/cnap-tech/akua/main/scripts/install.sh | sh

# From source (any platform with Rust toolchain)
cargo install --git https://github.com/cnap-tech/akua akua-cli
```

Or grab the prebuilt binary directly from [GitHub Releases][releases]
— every artefact ships with a SHA-256 checksum file.

Windows (Scoop, PowerShell), Arch AUR, and Docker images are queued
for `v0.1.1` — the release plumbing is in place, awaiting the next tag.

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
[playground]: https://cnap-tech.github.io/akua/
[hip-0026]: https://helm.sh/community/hips/hip-0026/
