# Getting Started

> Status: placeholder. This doc will be populated once the v4 CLI ships.

## Prerequisites (planned)

- Rust 1.75+ (for building from source)
- Node.js 20+ and pnpm 9+ (for TS packages)
- An OCI registry where you have push rights (optional for local dev)

## Installation (planned)

```bash
# Binary release (future)
curl -fsSL https://akua.sh/install.sh | sh

# Or via Cargo
cargo install akua-cli

# Or via npm (MCP server only)
npm install -g @akua/mcp
```

## First package (planned)

```bash
# Scaffold a new package around a Helm chart
akua pkg init --from bitnami/postgresql

# Preview with sample inputs
akua pkg preview --inputs '{"subdomain": "acme"}'

# Build it
akua pkg build

# Push to your OCI registry
akua pkg publish --to oci://ghcr.io/you/my-package
```

## In the meantime

This section will fill in as the v4 milestone completes. Follow the [roadmap](./roadmap.md) for progress.
