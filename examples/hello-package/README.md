# hello-package

The minimal Akua package: one Helm chart, two user-input fields.

## Files

- `package.yaml` — the package manifest (name, version, sources)
- `values.schema.json` — JSON Schema with `x-user-input` / `x-input` markers

## Commands

Print the dependency tree:

```bash
cargo run -p akua-cli -- tree --package examples/hello-package
```

```
hello-package 0.1.0 (1 sources)
  - nginx@18.1.0 as nginx-xxxx  [https://charts.bitnami.com/bitnami]
```

Resolve user inputs against the schema:

```bash
cargo run -p akua-cli -- preview \
  --package examples/hello-package \
  --inputs '{"config.adminEmail": "admin@example.com", "httpRoute.hostname": "My App!"}'
```

```json
{
  "config": { "adminEmail": "admin@example.com" },
  "httpRoute": { "hostname": "my-app.apps.example.com" }
}
```

The `hostname` input demonstrates:

- **`slugify: true`** — `"My App!"` → `"my-app"` (RFC 1123 DNS label)
- **`template: "{{value}}.apps.example.com"`** — wraps the slug into a full hostname

## Status

`tree` and `preview` are pure in-memory operations — no Helm binary, no
network. Rendering actual manifests lands when the Helm fetch/render stage
ships.
