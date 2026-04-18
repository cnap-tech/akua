# Getting Started

Akua is pre-alpha — no binary releases yet. Build from source.

## Prerequisites

- **Rust 1.83+** (matches KCL's MSRV — akua-core pulls it in)
- **Go 1.25+** — only to rebuild `crates/helm-engine-wasm/assets/helm-engine.wasm`
  (the embedded Helm template engine). Pre-built wasm is gitignored; a
  CI/release workflow will publish it alongside tagged versions.
- **[mise](https://mise.jdx.dev/) + [task](https://taskfile.dev/)** — recommended for pinned tool versions (`mise install` pulls everything listed in `.mise.toml`).

No `helm` CLI needed for the default render flow. No Docker needed unless
you want to spin up a local OCI registry for publish testing.

## Build from source

```bash
git clone https://github.com/cnap-tech/akua
cd akua
mise install                            # pulls rust, bun, task, helmfile, wasm-pack, …
task build:helm-engine-wasm             # one-time: compiles the embedded Helm wasm (~75 MB)
cargo build --release -p akua-cli
./target/release/akua --help
```

First `cargo build` takes a while (KCL + Helm deps are heavy); subsequent
builds in the same workspace are fast.

## Your first package

Use `examples/hello-package/` as a starting point — a minimal Helm-engine
package with two user-input fields:

```bash
cd examples/hello-package

# 1. Validate the schema + list customer-input fields
akua lint
# → schema ok — 2 user-input field(s)
#     - config.adminEmail
#     - httpRoute.hostname

# 2. Show the umbrella dependency structure
akua tree
# → hello-package 0.1.0 (1 sources)
#     - nginx@18.1.0 as nginx-xxxx  [https://charts.bitnami.com/bitnami]

# 3. Resolve inputs through the schema transforms (CEL)
akua preview --inputs '{
  "config.adminEmail": "admin@example.com",
  "httpRoute.hostname": "My App!"
}'
# → { "config": { "adminEmail": "admin@example.com" },
#     "httpRoute": { "hostname": "my-app.apps.example.com" } }

# 4. Build the chart (assembles umbrella + emits provenance)
akua build --out ../../dist/chart
# → wrote dist/chart/Chart.yaml + values.yaml
# → wrote dist/chart/.akua/metadata.yaml

# 5. Render to Kubernetes manifests (embedded Helm engine — no helm CLI)
akua render --package . --out ../../dist/chart --release demo --inputs '{
  "config.adminEmail": "admin@example.com",
  "httpRoute.hostname": "acme"
}' > ../../dist/manifests.yaml

# 6. Emit a SLSA v1 provenance attestation (unsigned predicate for cosign)
akua attest --chart ../../dist/chart --out ../../dist/attestation.json

# 7. Publish to an OCI registry (native, no helm CLI)
akua publish --chart ../../dist/chart --to oci://ghcr.io/you/my-pkg
# → pushed: ghcr.io/you/my-pkg/hello-package:0.1.0
# → digest: sha256:…
```

## Authoring a package

A package is a directory with:

- `package.yaml` — name, version, sources list, engine per source
- `values.schema.json` — JSON Schema with `x-user-input` / `x-input` extensions
- Engine-specific source files (a `.k` for KCL, `helmfile.yaml` for helmfile, or nothing extra for helm sources pointing at an existing chart)

Minimal `package.yaml`:

```yaml
name: my-package
version: 0.1.0
sources:
  - id: app
    engine: helm                   # or: kcl, helmfile
    chart:
      repoUrl: oci://ghcr.io/acme/charts
      chart: my-chart
      targetRevision: 1.0.0
```

Minimal `values.schema.json`:

```json
{
  "type": "object",
  "properties": {
    "subdomain": {
      "type": "string",
      "title": "Subdomain",
      "x-user-input": { "order": 10 },
      "x-input": {
        "cel": "value.lowerAscii() + '.apps.example.com'",
        "uniqueIn": "tenant.hostnames"
      }
    }
  },
  "required": ["subdomain"]
}
```

See `examples/` for more patterns (KCL-authored, helmfile-wrapped).

## Using `@akua/core-wasm` from the browser

The Rust core compiles to wasm-pack bindings consumable from Node or a
bundler like Vite/Webpack. Browser apps get the same schema extractor + CEL
evaluator the CLI uses — enabling **zero-network live preview** as
customers fill an install form.

```bash
# Build the wasm package
task wasm:build              # → packages/core-wasm/ (bundler target)
task wasm:build:node         # → node target
task wasm:smoke              # build + run the Node smoke test
```

Typical consumption:

```js
import * as akua from '@akua/core-wasm';

const fields = akua.extractUserInputFields(schema);
const resolved = akua.applyInputTransforms(fields, userInputs);
```

## Next steps

- [`docs/use-cases.md`](use-cases.md) — end-to-end user journeys (author → install → deploy) with ArgoCD YAML for both shared-chart and per-customer-chart OCI models
- [`docs/design-notes.md`](design-notes.md) — the *why*: positioning, invariants, trade-offs, determinism reality check
- [`docs/roadmap.md`](roadmap.md) — phase-by-phase status

File issues at [cnap-tech/akua](https://github.com/cnap-tech/akua) —
pre-alpha so expect rough edges.
