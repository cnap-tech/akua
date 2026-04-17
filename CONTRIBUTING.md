# Contributing to Akua

Thanks for your interest in Akua. The project is **pre-alpha** — nothing is stable, everything is in flux. Contribution guidelines will solidify as the project matures.

## Current status

Akua is being extracted from CNAP's internal chart generation service. Until the v4 API surface stabilizes, we're not actively accepting code contributions — PRs against a churning API are painful for everyone. We welcome:

- **Issues** — bug reports, feature requests, design feedback
- **Discussions** — design questions, architectural debates
- **RFCs / CEP comments** — feedback on the [CNAP Enhancement Proposal](https://github.com/cnap-tech/cnap/blob/main/internal/cep/20260417-chart-transformation-platform.md) that drives this project

## What we're working on

Track active work via the [v4 milestone](https://github.com/cnap-tech/akua/milestones). Top priorities:

1. Port CNAP's umbrella chart generation logic from Go/TS to Rust (`akua-core`)
2. Integrate Extism as the WASM plugin host
3. Pluggable `SourceFetcher` trait with implementations for server, CI, local, browser
4. `akua pkg build` / `akua pkg preview` CLI entry points
5. OCI push via `oras` or Rust-native OCI crate

## Development setup (when stabilized)

```bash
# Prerequisites
# - Rust 1.75+
# - Node.js 20+ with pnpm
# - wasm-pack (for WASM builds)

git clone git@github.com:cnap-tech/akua.git
cd akua

# Build Rust workspace
cargo build

# Install TS workspace deps
pnpm install

# Run tests
cargo test && pnpm test
```

## Code of Conduct

This project follows the [Contributor Covenant Code of Conduct](./CODE_OF_CONDUCT.md). Be kind.

## License

By contributing, you agree your contributions will be licensed under Apache License 2.0.
