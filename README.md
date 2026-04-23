<p align="center">
  <img src="assets/logo.png" alt="the akua packaging toolkit" width="600">
</p>

<p align="center">
  <b>The Unified Rendering Engine & K8s Config Packaging Toolkit</b>
</p>

<p align="center">
  <audio controls src="assets/theme.mp3">
    Your browser does not support the audio element.
  </audio>
</p>

<p align="center">
  <a href="https://github.com/cnap-tech/akua/actions/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/cnap-tech/akua/ci.yml?branch=main&label=CI&style=flat-square"></a>
  <a href="https://github.com/cnap-tech/akua/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/cnap-tech/akua?label=release&style=flat-square"></a>
  <a href="./LICENSE"><img alt="License" src="https://img.shields.io/badge/license-Apache--2.0-blue?style=flat-square"></a>
  <a href="#status"><img alt="Status" src="https://img.shields.io/badge/status-pre--alpha-orange?style=flat-square"></a>
</p>


## What is akua?

akua is a cloud-native packaging toolkit, shipped as a single Rust binary. Packages are authored in **KCL** — a typed configuration language with real types, functions, and imports. Existing Helm charts, kro RGDs, and Kustomize bases will be callable KCL functions, so the whole ecosystem works unchanged.

```bash
akua render --inputs inputs.yaml      # render to raw YAML
```

Designed agent-first: auto-detects Claude Code, Cursor, Codex, Gemini CLI, Goose, Amp, OpenCode, Cline, and 20+ more, emitting structured JSON on every verb. Ships a [skills library](skills/) conforming to the [Agent Skills Specification](https://agentskills.io).

> [!WARNING]
> **Pre-alpha.** The tree is mid-pivot. 13 verbs ship on `main` (`init`, `add`, `remove`, `tree`, `whoami`, `version`, `verify`, `check`, `lint`, `fmt`, `diff`, `inspect`, `render`); the full verb set landed in [`docs/cli.md`](docs/cli.md) is the target, not the current state. Don't build production workloads on this yet.

## Install

```sh
# from source (primary path today)
cargo install --git https://github.com/cnap-tech/akua akua-cli

# into your AI agent (universal)
npx skills install github:cnap-tech/akua/skills
```

Prebuilt binaries live on [GitHub Releases][releases]. Agent setup for Claude Code, Cursor, Codex, Gemini CLI, Goose, Amp, and 25+ others: [`docs/agent-usage.md`](docs/agent-usage.md).

## Quick start

A Package.k is plain KCL with three regions — imports, schema, body:

```kcl
import akua.ctx

schema Input:
    appName: str
    replicas: int = 2

input: Input = ctx.input()

resources = [{
    apiVersion: "v1"
    kind: "ConfigMap"
    metadata.name: input.appName
    data.replicas: str(input.replicas)
}]
```

Render it:

```sh
akua render --package ./Package.k --inputs inputs.yaml --out ./deploy
```

Full CLI reference: [`docs/cli.md`](docs/cli.md). Universal contract: [`docs/cli-contract.md`](docs/cli-contract.md). Runnable examples: [`examples/`](examples/).

## Architecture

Two crates:

- [`akua-core`](crates/akua-core) — the Rust library: CLI contract primitives, `akua.toml` / `akua.lock` parsers, `Package.k` loader, render output writer.
- [`akua-cli`](crates/akua-cli) — the `akua` binary. Every verb JSON-first, idempotent, typed exit codes — see [`docs/cli-contract.md`](docs/cli-contract.md).

KCL is the authoring language. Engine callables live under `akua.*`: `helm.template(...)` and `kustomize.build(...)` ship today (behind `--features engine-helm-shell` / `engine-kustomize-shell`); `pkg.render(...)` ships always for Package-of-Packages composition; `kro.rgd(...)` / `crossplane.composition(...)` arrive in later Phase B increments.

Deep dives:
[`docs/architecture.md`](docs/architecture.md) ·
[`docs/cli.md`](docs/cli.md) ·
[`docs/cli-contract.md`](docs/cli-contract.md) ·
[`docs/agent-usage.md`](docs/agent-usage.md) ·
[`docs/package-format.md`](docs/package-format.md) ·
[`docs/policy-format.md`](docs/policy-format.md) ·
[`docs/lockfile-format.md`](docs/lockfile-format.md) ·
[`docs/embedded-engines.md`](docs/embedded-engines.md) ·
[`docs/performance.md`](docs/performance.md) ·
[`skills/`](skills/) ·
[`examples/`](examples/) ·
[`docs/roadmap.md`](docs/roadmap.md)

## Status

What's shipped on `main`:

- 13 CLI verbs: `init`, `add`, `remove`, `tree`, `whoami`, `version`, `verify`, `check`, `lint`, `fmt`, `diff`, `inspect`, `render`.
- `akua.toml` + `akua.lock` parsers with round-trip tests against every example.
- `Package.k` loader with inputs wired via the `akua.ctx` stdlib (`input: Input = ctx.input()`).
- KCL plugin bridge: `akua.helm.template`, `akua.kustomize.build`, `akua.pkg.render` all callable from Packages. `helm`/`kustomize` gated behind their `engine-*-shell` features.
- Raw-YAML render writer with deterministic filenames + `sha256` hashes — same inputs produce byte-identical output.

What's next: [`docs/roadmap.md`](docs/roadmap.md).

## Security

See [SECURITY.md](SECURITY.md) for the threat model and vulnerability-disclosure process.

## Contributing

Pre-alpha means APIs churn. Small focused fixes (typos, doc clarity, test coverage, security findings) are always welcome; PRs against in-flight features may hit merge friction. See [CONTRIBUTING.md](CONTRIBUTING.md) and [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## Naming

"Akua" — Hawaiian for *divine spirit*; echoes **aqua**, water. Fits the cloud-native tradition: Docker loads the cargo, **Helm** steers the ship, **Harbor** stores what's shipped, **Kubernetes** (Greek *kubernḗtēs*, "helmsman") pilots the fleet. Akua is the current underneath — the flow that carries your sources, transforms them in motion, and delivers a sealed package to the harbor.

## License

[Apache-2.0](LICENSE).

[releases]: https://github.com/cnap-tech/akua/releases
