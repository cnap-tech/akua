# Changelog

All notable changes to Akua will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) once v1 ships.

## [Unreleased]

### Added

- Rust workspace: `akua-core`, `akua-cli`, `akua-wasm`, `helm-engine-wasm`
- TypeScript packages: `@akua/core-wasm` (from wasm-pack), `@akua/core`, `@akua/sdk`, `@akua/ui`
- Core pipeline: source/value/schema modules, CEL expressions, umbrella assembly, provenance, SLSA attestation
- Engines: `helm` (pass-through), `kcl` (native Rust via `kcl-lang`), `helmfile` (CLI shim)
- Embedded Helm v4 template engine (Go→wasip1 via wasmtime) — default render path, no `helm` CLI
- Native chart-dep fetcher (OCI + HTTP Helm repos) — replaces `helm dependency update`
- Native OCI publish (oci-client, Helm v4–compatible media types + annotations)
- Apache 2.0 license

### Status

Pre-alpha. No releases yet. API and schema churn expected.
