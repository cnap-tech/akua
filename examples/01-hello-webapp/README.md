# Example 01 — hello-webapp

The smallest possible akua Package. One local-path chart dep, an
`Input` schema with two fields, raw-manifest output. Teaches the
three regions of `package.k` — imports, schema, body — and the
`charts.*` typed-import pattern that Phase 2a landed.

Read this first. Every other example adds exactly one concept over
this one.

## Layout

```
01-hello-webapp/
├── akua.toml                  declared deps (one local-path chart)
├── akua.lock                  digest ledger (machine-maintained)
├── package.k                  the Package itself
├── inputs.yaml                sample inputs satisfying the Input schema
├── vendor/nginx/              the vendored chart — see Phase 2b for OCI pulls
│   ├── Chart.yaml
│   ├── values.yaml
│   └── templates/
└── README.md
```

## The three regions in `package.k`

1. **Imports** — `akua.ctx` (reads user inputs) and `charts.nginx`
   (resolved from `akua.toml` — a per-render stub module akua's chart
   resolver synthesizes, exposing `template`/`Values`/`TemplateOpts`).
   No `import akua.helm` at the call site — the stub dispatches.
2. **Schema** — `Input` with two fields. Both have defaults; docstrings
   will become UI labels once the schema-extraction path ships.
3. **Body** — one call to `nginx.template(nginx.TemplateOpts{...})`
   wiring public schema into chart values. Resources are aggregated
   into top-level `resources`. akua writes one YAML file per resource
   under `--out`.

## Run

```sh
akua add                                 # resolve deps → writes akua.lock
akua render --inputs inputs.yaml         # render to ./deploy/
ls deploy/                               # 000-deployment-hello.yaml, 001-service-hello.yaml
```

Under `--strict`, akua rejects raw-string chart paths — every chart
must be declared in `akua.toml` and imported as `charts.<name>`:

```sh
akua render --strict --inputs inputs.yaml
```

## Vendored chart vs OCI pull

This example vendors nginx into `vendor/nginx/` so the Package is
self-contained and rendering works offline. To point at a registry
instead, swap the `akua.toml` dep for:

```toml
[dependencies]
nginx = { oci = "oci://registry-1.docker.io/bitnamicharts/nginx", version = "18.2.0" }
```

akua pulls the chart into `$XDG_CACHE_HOME/akua/oci/` on first
`akua add` / `akua render`, verifying the blob digest against
`akua.lock` on subsequent renders. See Phase 2b in `docs/roadmap.md`.

## Local fork override

While iterating on a chart, point a real `oci://` dep at a local
clone without losing the canonical source-of-record:

```toml
nginx = { oci = "oci://registry-1.docker.io/bitnamicharts/nginx", version = "18.2.0", replace = { path = "../nginx-fork" } }
```

`akua.lock` still records the `oci://` digest; files resolve from
`../nginx-fork`. Drop the `replace` clause to switch back.

## See also

- [package-format.md](../../docs/package-format.md) — canonical Package spec
- [cli.md `render`](../../docs/cli.md) — render verb + flags
- [roadmap.md Phase 2](../../docs/roadmap.md) — chart dep resolver design
- [02-webapp-postgres/](../02-webapp-postgres/) — next example: cross-source wiring
