# Changelog

All notable changes to Akua will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
once v1 ships. Until then, `@akua-dev/sdk` versions bump independently of the
Rust workspace; breaking changes to `v1alpha1` data shapes trigger a
minor bump in the SDK.

> **Note:** the SDK was published as `@akua/sdk` on JSR through 0.5.0.
> Starting with 0.6.0 it ships as `@akua-dev/sdk` on npm — JSR's 20 MB
> single-file/total-package cap is incompatible with the bundled napi
> addon (~129 MB compressed across the per-platform packages).

## [0.7.0] — 2026-04-28

The `pkg.render` round: a synchronous engine plugin that mirrors
`helm.template` / `kustomize.build`, plus the supporting work to
make composition first-class (path-deps + OCI Akua-package deps,
budget guards, structured error codes, a worked install-as-Package
example).

### Added

- **`pkg.render` is a synchronous engine plugin.** Returns a real
  `[{str:}]` list, not a deferred sentinel. List-comprehension
  patches (`[r | overlay for r in pkg.render(...)]`), filter
  expressions, and slicing all work natively. Requires the akua
  KCL fork (`cnap-tech/kcl@akua-wasm32`, commit `d584c0bc`) which
  copies `PLUGIN_HANDLER_FN_PTR` out of its mutex before invoking
  the plugin callback so reentrant KCL eval no longer deadlocks.
- **OCI Akua-package deps.** `[dependencies] upstream = { oci = "..." }`
  resolves to a `KclModule` even when the artifact carries
  `package.k` (no `kcl.mod`) and `dev.akua.*` annotations.
- **Budget header for `pkg.render`.** `BudgetSnapshot { deadline,
  max_depth }` propagated through the render stack and checked
  before nested invocations. Default depth cap is 16; outermost
  callers can install an explicit deadline via
  `RenderScope::enter_with_budget`. Catches recursive-composition
  runaway.
- **Structured error codes** for plugin failures:
  `E_RENDER_CYCLE`, `E_RENDER_BUDGET_DEPTH`,
  `E_RENDER_BUDGET_DEADLINE`. Routed through a marker→code lookup
  table.
- **`examples/11-install-as-package/`** — worked install-as-Package
  shape: outer Package overlays a tenant label, filters out a kind,
  and appends an extras ConfigMap on top of `pkg.render`'d upstream.
- **`renovate.json`** — pre-1.0 cargo bumps no longer batch into a
  single PR.

### Changed

- Render-worker rebuild trigger now watches `akua-render-worker/src`
  + `akua-core/src` and emits a `cargo:warning=` when the staged
  `.cwasm` is stale.
- `akua init .` derives the package name from `basename($PWD)`
  instead of writing `name = "."`.
- `E_PATH_ESCAPE` errors now carry a `hint` field with both
  remediations (vendor under the Package or declare in
  `akua.toml`).
- `akua render --debug` (under `--json`) emits `evalResult`
  alongside the summary — the post-eval resources list before
  YAML normalization.

### Removed

- The `pkg.render` deferred-sentinel mechanism + the
  `E_PKG_RENDER_PATCH_UNSUPPORTED` fail-loud arm. Patching the
  return is now native, so the workaround retired.

## @akua-dev/sdk — [0.6.0] — 2026-04-27

The SDK moves from JSR to npm and renames to `@akua-dev/sdk`. This is
also the version that pivots from the chart-tooling shape (`pullChart`,
`packChart`, `pullChartStream`, `inspectChartBytes` — last shipped as
`@akua/sdk` 0.5.0) to a CLI-mirror shape: an `Akua` class whose methods
map 1:1 to the binary's verbs. Same `--json` envelopes, same typed
errors, all in-process via a bundled napi addon.

### Added

- `Akua` class — every shipping CLI verb is a method:
  - **Read-only:** `version`, `whoami`, `lint`, `fmt`, `check`,
    `tree`, `diff`, `export`, `verify`.
  - **Render:** `render({ package, inputs, out, dryRun, strict,
    offline })` returns the same `RenderSummary` envelope the CLI
    emits, byte-for-byte. `renderSource({ source | package,
    inputs })` returns rendered YAML directly for source-string
    consumers.
- Full feature parity with the binary in-process — Helm + Kustomize
  engines, OCI fetch, cosign verify, JSON Schema / OpenAPI export.
  No `akua` binary on `$PATH`, no shell-out. Native addon
  (`@akua-dev/native`, per-platform via Node-API / napi-rs) for the
  feature-rich path; the existing `akua-wasm` bundle stays for the
  pure-KCL fast path and browser targets.
- `renderSource({ source, package, packageFilename, packageDir,
  inputs })` — accepts raw KCL source or an on-disk Package; engine
  callouts (`helm.template`, `kustomize.build`) work transparently
  when a `packageDir` is provided.
- Typed-error routing across the napi boundary: thrown errors
  preserve the `code` (`E_PACKAGE_MISSING`, `E_RENDER_KCL`, …) so
  `instanceof AkuaUserError` / `AkuaSystemError` etc. continues to
  work.

### Removed

- The chart-tooling surface (`pullChart`, `pullChartStream`,
  `pullChartCached`, `packChart`, `packChartStream`,
  `inspectChartBytes`, `streamTgzEntries`, `unpackTgz`,
  `@akua/sdk/cache`, `SsrfError`). Subsumed by `Akua.render`'s
  in-process resolver, which ships the same OCI fetch + digest
  verify + cosign check as the CLI uses.
- `new Akua({ binary: '...' })` — the binary path option is gone
  (no shell-out anymore). `new Akua()` is the only valid
  construction.

### Migration from 0.5.0

The two surfaces don't overlap; consumers of the chart-tooling
APIs need to rewrite. The 0.5.0 line stays installable on JSR for
legacy callers; new code should use `0.6.0` and the `Akua`-class
shape.

## @akua/sdk — [0.4.0] — 2026-04-18

### Changed

- `pullChart` / `pullHelmHttpChart` stream-consume the response body
  with a running per-chunk `maxBytes` guard; reader is cancelled on
  overrun so a hostile server can't keep pushing bytes after we've
  given up.
- OCI path: preflight `Content-Length` against `maxBytes` before
  opening the blob stream (parity with the HTTP Helm path).

### Security

- **P2** `tar.ts` lint errors resolved (`Uint8Array<ArrayBuffer>` TS
  variance). No runtime change — but removes noise that was hiding
  real errors in `bun run lint`.
- **P2** Native `Error.cause` throughout (was `cause_` underscore
  field in a couple of subclasses).

## @akua/sdk — [0.3.0] — 2026-04-18

### Added

- `pullChartStream(ref, options)` — streaming variant returning
  `ReadableStream<Uint8Array>`; pipe straight into `inspectChartBytes`
  / Convex / fetch bodies without buffering.
- `@akua/sdk/cache` (Node-only subpath export) — `pullChartCached(ref)`
  shares `$XDG_CACHE_HOME/akua/v1/` with the CLI. Respects
  `AKUA_NO_CACHE` and `AKUA_CACHE_DIR`.
- `SsrfError` + SSRF guard: pull hosts resolving to loopback / RFC1918
  / link-local IPs (incl. AWS metadata at `169.254.169.254`) are
  rejected. Bypass with `AKUA_ALLOW_PRIVATE_HOSTS=1`.
- `streamTgzEntries` / `unpackTgz` options: `maxEntries` (20 000),
  `maxTotalBytes` (500 MB), `maxEntryBytes` (100 MB) — gzip-bomb caps
  mirroring the Rust side.

### Changed

- Replace hand-rolled YAML parser in `tar.ts` with the `yaml` npm
  package. Fixes Helm-repo `index.yaml` compact-list edge cases that
  tripped the narrow parser (Bitnami, Jetstack).
- Consolidated auth helpers into `src/auth.ts` —
  `credentialsToAuthHeader`, `toHost`, `base64Encode`. Removes
  duplicate logic across `oci.ts` / `helm-http.ts` / `docker-config.node.ts`.
- `AkuaError` now uses native ES2022 `cause` (was a `cause_`
  underscore field).
- Per-repo `index.yaml` fetch cache — a build pulling N deps from the
  same Helm HTTP repo issues one index fetch instead of N.
- `packChart` scrubs `dependencies[].repository` starting with
  `file://` before emitting `Chart.yaml` (matches CLI `akua package`).
- `docker-credential-*` helpers: validate helper name regex,
  drain stderr to prevent pipe stalls, enforce 5 s timeout.
- `pullChart` / `pullHelmHttpChart` now stream-consume response
  bodies with a running `maxBytes` guard — a server advertising an
  oversized Content-Length can't force a full buffer allocation.

### Security

- **P0** Tar extraction — reject symlink + hard-link entries in Rust
  `unpack_chart_tgz`. Prevents arbitrary file read via
  `akua inspect` on a malicious chart whose entry points at
  `/etc/passwd`.
- **P0** `engine-helmfile` removed from default cargo features.
  Helmfile's Go-template `exec` / `readFile` / `requiredEnv` functions
  let an attacker package run arbitrary commands at build time. Opt in
  only for trusted packages.
- **P1** SSRF guard across Rust + SDK — private-range IP literals
  rejected; `reqwest` redirect policy re-validates each hop.
- **P1** `helm-cli` render engine now pre-populates `charts/` via
  akua's audited fetcher and skips `helm dependency update`. Helm
  never makes network calls for an untrusted package.
- **P1** `kcl.entrypoint` + `helmfile.path` confined to the package
  directory (no absolute paths, no `..`).
- **P2** Redacted `Debug` impls for `OciAuth` / `RegistryCredentials` /
  `BasicAuth` — prevents accidental `?auth` tracing leaks.
- **P2** Strict OCI hostname validation (no userinfo, fragments,
  queries).
- **P2** Strict helm-layer media-type enforcement — reject manifests
  without the canonical `application/vnd.cncf.helm.chart.content.v1.tar+gzip`
  layer instead of falling back to `layers[0]`.
- **P2** URL userinfo redaction in error messages. Prevents
  `oci://user:pass@host/...` userinfo from leaking into log lines.
- **P2** Content-Length preflight on OCI + HTTP Helm pulls; capped
  `Vec::with_capacity` preallocation (4 MB max) — a server spoofing
  `Content-Length: u64::MAX` can't force a 100 MB up-front allocation.
- **P2** CEL evaluation: source length capped at 8 KB, 5 s wall-clock
  timeout. Malicious `x-input.cel` can't pin the worker thread.
- **P2** Cache LRU eviction (`AKUA_MAX_CACHE_BYTES`, default 5 GB).
- **P2** `--helm-bin` must be an absolute path when `--engine=helm-cli`
  — prevents `$PATH` shadowing by a writable directory.
- **P2** `manifest.schema` path validated at load time (relative,
  no `..`).
- **P2** Per-call `FetchOptions` override for `AKUA_MAX_*` limits —
  multi-tenant workers no longer share process-global caps.
- **P2** Migrated from deprecated `serde_yaml` to `serde_yml` (the
  maintained fork).

### Docs

- New top-level `SECURITY.md` — threat model, fixed attack surfaces,
  remaining caveats, reporting process.
- Rewrote `packages/sdk/README.md` — worked examples for `pullChart`,
  `packChart`, `dockerConfigAuth`, streaming, cache, safety limits.
- Top-level `README.md` updated: SDK shipped (was "planned"), project
  structure reflects actual layout.

## @akua/sdk — [0.2.0] — 2026-04-18

### Added

- `pullChart` dispatches on scheme: `oci://` (existing) and
  `https://` / `http://` (new, Helm HTTP repos).
- `packChart` options: `valuesSchema`, `metadata`, `signal` — emit
  `values.schema.json` / `.akua/metadata.yaml` alongside
  `Chart.yaml` / `values.yaml`.
- `dockerConfigAuth()` Node helper — reads `~/.docker/config.json`,
  supports `auth`, `identitytoken`, `credHelpers`, `credsStore`.
- `AkuaError` base class; `OciPullError`, `TarError`, `HelmHttpError`,
  `DockerConfigError`, `WasmInitError` all extend it.
- `buildMetadata(sources, fields?, options?)` + `packChart`
  `metadata` option. Honours `SOURCE_DATE_EPOCH` for reproducible
  `buildTime`.
- `dependencyToOciRef(dep)` helper.
- `ChartYaml` type gained `appVersion`, `keywords`, `home`, `sources`,
  `icon`, `annotations`, `maintainers` (optional).

### Changed

- `AbortSignal` wired through `packChart` and `packChartStream`.

## @akua/sdk — [0.1.0] — 2026-04-18

Initial published SDK.

### Added

- `pullChart(ref, options)` — pure-TS OCI pull, no `helm` binary.
- `unpackTgz`, `streamTgzEntries`, `packTgz`, `packTgzStream`,
  `inspectChartBytes`.
- `buildUmbrellaChart`, `mergeSourceValues`, `mergeValuesSchemas`,
  `extractUserInputFields`, `applyInputTransforms`,
  `validateValuesSchema`, `hashToSuffix`.
- `packChart` + `packChartStream`.
- Node (`@akua/sdk`) + browser (`@akua/sdk/browser`) entries.
- Published to JSR via GitHub Actions OIDC.

## CLI — [0.1.0] — 2026-04-27

First tagged release of the `akua` binary. Substrate-shape only — no
curated catalog, no cluster control plane. Ten green examples render
deterministically; 26 verbs implement the universal CLI contract.

### Added

**Authoring + render**
- KCL-typed Packages: `package.k` with `import` + `schema` +
  `resources` regions, published as signed OCI artifacts.
- `akua render` — wasmtime-sandboxed evaluation. Engines (Helm v4,
  Kustomize) compiled to `wasm32-wasip1` and hosted inside akua's own
  wasmtime — no `$PATH`, no shell-out, no ambient filesystem.
- `akua export` — emit the Package's `Input` schema as JSON Schema
  2020-12 or OpenAPI 3.1. Field docstrings become `description`;
  `@ui(...)` decorators become `x-ui` extensions for form renderers
  (rjsf, JSONForms) and admission-webhook validators.
- `@ui(...)` schema decorators on `Input` attributes (`order`,
  `group`, `widget`, `min`, `max`, `placeholder`, …). Decorator
  arguments project losslessly into the exported schema; render
  strips them before handing source to KCL's resolver.
- Determinism invariant: same inputs + same `akua.lock` + same akua
  version → byte-identical output. No `now()`, no `random()`, no env
  reads in the render pipeline.

**Dependency + lockfile shape**
- `akua.toml` + `akua.lock` — human intent + digest-pinned ledger.
  Three source kinds: `oci`, `git`, `path`. `[replace]` sections for
  vendor + path overrides.
- KCL ecosystem support — pull `oci://ghcr.io/kcl-lang/*` packages
  alongside Helm charts. `import k8s.api.apps.v1` resolves against
  the upstream KCL bundle inside the sandbox.

**Verbs (26 shipped)** —
`init` · `whoami` · `version` · `verify` · `render` · `add` · `dev` ·
`test` · `tree` · `pull` · `publish` · `sign` · `update` · `lock` ·
`push` · `repl` · `pack` · `remove` · `diff` · `check` · `inspect` ·
`lint` · `fmt` · `cache` · `auth` · `export`. Universal contract:
every verb supports `--json`, `--plan`, `--timeout`,
`--idempotency-key`; typed exit codes 0–6; structured-error stderr.

**Agent-first surface**
- Auto-detection of Claude Code, Cursor, Codex, Gemini CLI, Goose,
  Amp, OpenCode, Cline, and 25+ other agents. Detected sessions
  auto-enable `--json`, `--no-color`, `--no-progress`,
  `--no-interactive`.
- Skill manifests under `skills/` conforming to the [Agent Skills
  Specification](https://agentskills.io).

**Signing + attestation**
- `akua publish` emits cosign signatures (ECDSA P-256 keyed) and
  SLSA v1 predicates by default; consumers verify on pull. Air-gap
  flow: `akua pack` → `akua sign` → `akua verify --tarball`.

**SDK**
- `@akua/sdk` (`packages/sdk`) — in-process WASM via `akua-wasm`
  crate, `wasm32-unknown-unknown` target. Verbs callable without
  spawning the binary: `version`, `whoami`, `render`, `renderSource`,
  `check`, `lint`, `fmt`, `inspect`, `tree`, `verify`, `diff`,
  `export`. Same shapes the CLI emits — typed via `ts-rs`-generated
  TypeScript types and `schemars`-emitted JSON Schemas.

### Security

- Wasmtime sandbox is structural — no `--unsafe-host` escape hatch,
  no shell-out fallback. Memory / epoch / wall-clock caps enforced
  per render; capability-model preopens scope filesystem access.
- Adversarial test suite: zip-bomb resistance, path traversal,
  symlink escape, `PATH=/nonexistent` invariance, fork-bomb caps —
  all green.
- Every Rust-side hardening from the SDK 0.3.0 entry applies to the
  CLI: tar-extraction symlink rejection, SSRF guard, OCI media-type
  strictness, Content-Length preflight, `engine-helmfile` opt-in,
  `helm-cli` opted out of `helm dependency update`.

## Status

Alpha. v0.1.0 is the first tagged release. Stable contracts: the
26-verb CLI surface, the universal flag/exit-code contract, the
WASM-backed SDK methods, the wasmtime sandbox invariant. Anything in
[`docs/roadmap.md`](docs/roadmap.md) under Phase 5+ may change before
v0.2.0. Safe for CI and agent workflows today; pin akua versions for
production rollouts.
