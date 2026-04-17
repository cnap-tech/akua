# hello-package

The minimal Akua package: one JSON Schema with two user-input fields.

```bash
# From the repo root:
cargo run -p akua-cli -- preview \
  --package examples/hello-package \
  --inputs '{"config.adminEmail": "admin@example.com", "httpRoute.hostname": "My App!"}'
```

Expected output (resolved values):

```json
{
  "config": { "adminEmail": "admin@example.com" },
  "httpRoute": { "hostname": "my-app.apps.example.com" }
}
```

The `hostname` input demonstrates:

- **`slugify: true`** — `"My App!"` → `"my-app"` (RFC 1123 DNS label)
- **`template: "{{value}}.apps.example.com"`** — wraps the slug into a full hostname

`preview` reads `values.schema.json`, extracts user-input fields, and applies
transforms. It does not render Helm manifests yet.
