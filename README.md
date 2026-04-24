
<p align="center">
<img alt="akua mascot" height="256" src="assets/logo.png" />
</p>

<h1 align="center">the Akua packaging toolkit</h1>

<p align="center">
  <a href="https://github.com/cnap-tech/akua/actions/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/cnap-tech/akua/ci.yml?branch=main&label=CI&style=flat-square"></a>
  <a href="https://github.com/cnap-tech/akua/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/cnap-tech/akua?label=release&style=flat-square"></a>
  <a href="./LICENSE"><img alt="License" src="https://img.shields.io/badge/license-Apache--2.0-blue?style=flat-square"></a>
  <a href="#status"><img alt="Status" src="https://img.shields.io/badge/status-alpha-yellow?style=flat-square"></a>
</p>

## What is akua?

akua is a cloud-native packaging toolkit, shipped as a single Rust binary. Packages are authored in **KCL** — a typed configuration language with real types, functions, and imports. Existing Helm charts and Kustomize bases are callable KCL functions (`helm.template(...)`, `kustomize.build(...)`), so the whole ecosystem works unchanged — no shell-out, no `$PATH` dependency, every render runs inside a wasmtime sandbox.

```bash
akua render --inputs inputs.yaml      # render to raw YAML
```

Designed agent-first: auto-detects Claude Code, Cursor, Codex, Gemini CLI, Goose, Amp, OpenCode, Cline, and 20+ more, emitting structured JSON on every verb. Ships a [skills library](skills/) conforming to the [Agent Skills Specification](https://agentskills.io).

## Install

```sh
# from source
cargo install --git https://github.com/cnap-tech/akua akua-cli

# TypeScript SDK — in-process via WASM, no binary required for pure-compute verbs
bun add jsr:@akua/sdk

# into your AI agent (universal)
npx skills install github:cnap-tech/akua/skills
```

Prebuilt binaries live on [GitHub Releases][releases]. Agent setup for Claude Code, Cursor, Codex, Gemini CLI, Goose, Amp, and 25+ others: [`docs/agent-usage.md`](docs/agent-usage.md).

## Quick start

A Package.k is plain KCL — a typed schema plus the resources it emits:

```kcl
schema Input:
    appName: str
    replicas: int = 2

input: Input = option("input") or Input {}

resources = [{
    apiVersion: "v1"
    kind: "ConfigMap"
    metadata.name: input.appName
    data.replicas: str(input.replicas)
}]
```

Render it:

```sh
akua render --package ./package.k --inputs inputs.yaml --out ./deploy
```

Full CLI reference: [`docs/cli.md`](docs/cli.md). Universal contract: [`docs/cli-contract.md`](docs/cli-contract.md). Runnable examples: [`examples/`](examples/).

## What ships in v0.1.0

**26 CLI verbs** — `add`, `auth`, `cache`, `check`, `dev`, `diff`, `fmt`, `init`, `inspect`, `lint`, `lock`, `pack`, `publish`, `pull`, `push`, `remove`, `render`, `repl`, `sign`, `test`, `tree`, `update`, `vendor`, `verify`, `version`, `whoami`. Every verb emits `--json`, uses typed exit codes, and honors the universal flags in [`docs/cli-contract.md`](docs/cli-contract.md). Verbs in the target roadmap that don't ship yet: `deploy`, `policy`, `query`, `trace`, `audit`, `infra`, `export`, `bench`, `attest`, `eval`.

**Sandboxed by default.** Every render runs inside a wasmtime sandbox with memory / epoch / filesystem-capability caps — no shell-out, no `$PATH` binary needed, untrusted Packages are safe on shared hosts. Proven by an adversarial integration suite (memory bomb, epoch exhaustion, path escape, symlink escape, import escape, plugin panic boundary). See [`docs/security-model.md`](docs/security-model.md).

**Embedded engines.** Helm v4 and Kustomize compiled to `wasm32-wasip1` and hosted inside akua itself — `helm.template(...)` and `kustomize.build(...)` work with no `helm` or `kustomize` binaries on your machine. See [`docs/embedded-engines.md`](docs/embedded-engines.md).

**Signed + attested distribution.** `akua publish` emits cosign signatures + SLSA v1 attestations by default; `akua verify` walks the chain. ECDSA P-256, keyed cosign (keyless deferred to v0.3).

**TypeScript SDK on JSR.** `@akua/sdk` ships WASM-backed in-process verbs (`check`, `lint`, `fmt`, `inspect`, `tree`, `diff`, `renderSource`) — no `akua` binary required. File-backed / network-dependent verbs still shell out to the CLI. Node / Deno / Bun supported today; browser in v0.2.0.

**Determinism.** Same inputs + same lockfile + same akua version → byte-identical output. No `now()`, no `random()`, no env reads inside the render pipeline. Every green example has committed `rendered/` golden output that CI verifies on every change.

## Architecture

Seven workspace crates + one TypeScript SDK:

- [`akua-core`](crates/akua-core) — CLI contract primitives, `akua.toml` + `akua.lock` parsers, `Package.k` loader, render output writer, resolver, cosign. Pure library; 265 tests.
- [`akua-cli`](crates/akua-cli) — the `akua` binary. Thin envelopes that shell-out to `akua-core`; 182 lib tests + 30 integration tests.
- [`akua-render-worker`](crates/akua-render-worker) — the `wasm32-wasip1` module the CLI's sandbox hosts via wasmtime.
- [`akua-wasm`](crates/akua-wasm) — the `wasm32-unknown-unknown` module `@akua/sdk` loads in-process via wasm-bindgen.
- [`engine-host-wasm`](crates/engine-host-wasm) — shared wasmtime Engine + Session abstraction reused by helm/kustomize.
- [`helm-engine-wasm`](crates/helm-engine-wasm), [`kustomize-engine-wasm`](crates/kustomize-engine-wasm) — Go engines compiled to `wasm32-wasip1`, bundled into the akua binary.
- [`packages/sdk`](packages/sdk) — the `@akua/sdk` TypeScript package on JSR.

Deep dives:
[`docs/architecture.md`](docs/architecture.md) ·
[`docs/cli.md`](docs/cli.md) ·
[`docs/cli-contract.md`](docs/cli-contract.md) ·
[`docs/agent-usage.md`](docs/agent-usage.md) ·
[`docs/package-format.md`](docs/package-format.md) ·
[`docs/lockfile-format.md`](docs/lockfile-format.md) ·
[`docs/embedded-engines.md`](docs/embedded-engines.md) ·
[`docs/security-model.md`](docs/security-model.md) ·
[`docs/sdk.md`](docs/sdk.md) ·
[`skills/`](skills/) ·
[`examples/`](examples/) ·
[`docs/roadmap.md`](docs/roadmap.md)

## Status

**Alpha** — v0.1.0 is the first tagged release. The sandbox invariant, 26-verb CLI surface, and the 7 WASM-backed SDK methods are stable contracts; anything listed in the roadmap under Phase 5+ may change before v0.2.0. Safe to build on for CI/agent workflows today; large production rollouts should pin akua versions and track the [roadmap](docs/roadmap.md).

Full history: [`CHANGELOG.md`](CHANGELOG.md). What's next: [`docs/roadmap.md`](docs/roadmap.md).

## Security

See [`SECURITY.md`](SECURITY.md) for the threat model and vulnerability-disclosure process. Render-path invariant: **no shell-out, ever** — every engine runs inside wasmtime with memory / epoch / filesystem-capability caps; see [`docs/security-model.md`](docs/security-model.md) for the full model + adversarial-test catalogue.

## Contributing

Small focused changes (typos, doc clarity, test coverage, security findings) are always welcome. For larger changes, open an issue first so we can align on shape. See [`CONTRIBUTING.md`](CONTRIBUTING.md) and [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md).

## Naming

"Akua" — Hawaiian for *divine spirit*; echoes **aqua**, water. Fits the cloud-native tradition: Docker loads the cargo, **Helm** steers the ship, **Harbor** stores what's shipped, **Kubernetes** (Greek *kubernḗtēs*, "helmsman") pilots the fleet. Akua is the current underneath — the flow that carries your sources, transforms them in motion, and delivers a sealed package to the harbor.

## License

[Apache-2.0](LICENSE).

[releases]: https://github.com/cnap-tech/akua/releases
