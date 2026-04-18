# Changelog

All notable changes to Akua will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
once v1 ships. Until then, `@akua/sdk` versions bump independently of the
Rust workspace; breaking changes to `v1alpha1` data shapes trigger a
minor bump in the SDK.

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
  `extractInstallFields`, `applyInstallTransforms`,
  `validateValuesSchema`, `hashToSuffix`.
- `packChart` + `packChartStream`.
- Node (`@akua/sdk`) + browser (`@akua/sdk/browser`) entries.
- Published to JSR via GitHub Actions OIDC.

## CLI — [Unreleased]

### Added (since last release)

- `akua init` — scaffolds `package.yaml` + `values.schema.json` +
  `README.md`.
- `akua inspect --oci` — output now includes `ociManifestDigest`
  (single HEAD request for upstream-change detection).
- Stable `phase` field on all top-level log events in JSON format.

### Security

- See 0.3.0 entry above — every Rust-side hardening applies to the
  CLI too.

## Status

Pre-alpha. `@akua/sdk` has a published API surface frozen for
`v1alpha1`. Rust workspace is still on `0.0.0` internally; binary
releases pending (see [roadmap.md](docs/roadmap.md)).
