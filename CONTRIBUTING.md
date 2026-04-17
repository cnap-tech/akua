# Contributing to Akua

Thanks for your interest. Akua is **pre-alpha** — APIs, schemas, and CLI
surfaces are all in flux. Small focused fixes are welcome; big
architectural PRs are probably better raised as issues first so we can
discuss direction.

## Ways to help

- **Issues** — bug reports, feature requests, design feedback
- **Docs** — `docs/`, example READMEs, the top-level `README.md`; these drift fastest
- **Test coverage** — especially for engines + CEL expression edge cases
- **Regression coverage on real charts** — `akua render` against popular Helm charts from ArtifactHub; file issues for any that mis-render

## Development setup

Prerequisites are managed via [mise](https://mise.jdx.dev/) —
`mise install` pulls everything listed in `.mise.toml` (Rust, Go, bun,
task, helmfile, wasm-pack, etc.) at pinned versions.

```bash
git clone git@github.com:cnap-tech/akua.git
cd akua
mise install

# One-time: build the embedded Helm template engine wasm
task build:helm-engine-wasm

# Full workspace build + tests
cargo build --workspace
cargo test --workspace

# WASM bindings + smoke test
task wasm:smoke
```

First `cargo build` takes ~3.5 min (KCL + Helm dep trees are heavy).
Subsequent builds are fast.

## Running the examples

```bash
cargo run -p akua-cli -- tree --package examples/hello-package
cargo run -p akua-cli -- render --package examples/hello-package --out dist/chart --release demo
```

## What's landed / what's next

See [`docs/roadmap.md`](docs/roadmap.md). Phases 0–7 landed: Akua renders
end-to-end as a single binary with no external CLI deps in the default
flow. Upcoming: install UI reference, Package Studio IDE, upstream HIP
proposals.

## Code of Conduct

This project follows the [Contributor Covenant Code of Conduct](./CODE_OF_CONDUCT.md). Be kind.

## License

By contributing, you agree your contributions will be licensed under
Apache License 2.0.
