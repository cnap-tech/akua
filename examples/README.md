# Examples

Reference packages exercising Akua's engines end-to-end.

| Example | Engine | What it shows |
|---|---|---|
| **`hello-package/`** | `helm` | Minimal package wrapping a Bitnami nginx chart. Two `x-user-input` fields + CEL transforms (slugify + hostname template). The "hello world" of Akua. |
| **`kcl-package/`** | `kcl` | Package authored in [KCL](https://kcl-lang.io/) as a `.k` program. Native Rust evaluator embeds the KCL compiler; output wrapped as a static Helm subchart. |
| **`helmfile-package/`** | `helmfile` | Package wrapping an existing [helmfile](https://github.com/helmfile/helmfile) for migration. Shells to `helmfile template` at build time; output wrapped as a static Helm subchart. |

Each example is runnable from the repo root:

```bash
# Resolve inputs through the schema
cargo run -q -p akua-cli -- preview --package examples/hello-package \
  --inputs '{"config.adminEmail":"ops@acme.corp","httpRoute.hostname":"acme"}'

# Assemble the chart
cargo run -q -p akua-cli -- build --package examples/hello-package --out dist/chart

# Render to Kubernetes manifests via the embedded Helm engine
cargo run -q -p akua-cli -- render --package examples/hello-package --out dist/chart \
  --release demo \
  --inputs '{"config.adminEmail":"ops@acme.corp","httpRoute.hostname":"acme"}'

# Publish to an OCI registry (native push, no helm CLI)
cargo run -q -p akua-cli -- publish --chart dist/chart --to oci://ghcr.io/you/my-pkg
```

See each example's own `README.md` for engine-specific notes and the
determinism caveats (helmfile in particular).
