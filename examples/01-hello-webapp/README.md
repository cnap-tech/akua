# Example 01 — hello-webapp

The smallest possible akua Package. One Helm chart, one schema input (plus three with defaults), raw-manifest output. Teaches the four regions of `package.k` — imports, schema, body, outputs — and the two patterns for UI hints (docstrings + `@ui` decorators).

Read this first. Every other example adds exactly one concept over this one.

## Layout

```
01-hello-webapp/
├── akua.mod          declared deps (one Helm chart — upstream nginx)
├── akua.sum          digest + signature ledger (machine-maintained)
├── package.k         the Package itself
├── inputs.yaml       sample inputs satisfying the Input schema
└── README.md
```

## The four regions in `package.k`

1. **Imports** — `akua.helm` (engine callable), `akua.ui` (decorator home), `charts.nginx` (resolved from `akua.mod`).
2. **Schema** — `Input` with four fields. `hostname` is required; `name`, `replicas`, `tls` have defaults. Docstrings become UI labels; `@ui` decorators add ordering, groups, placeholders, widget hints.
3. **Body** — one call to `helm.template(...)` wiring our public schema into the chart's native values. No fork of the chart needed.
4. **Outputs** — a single raw-manifest output written to `./rendered`.

## Run

```sh
akua add                                 # resolve deps → writes akua.sum
akua render --inputs inputs.yaml         # render to ./rendered/
ls rendered/                             # deployment.yaml, service.yaml, ingress.yaml, ...
```

## UI hints — how the same file drives a customer form

Because every schema field is documented via docstrings and decorated via `@ui`, a consumer (install UI, Package Studio, rjsf form) can render a well-laid-out form from nothing but the Package:

```sh
akua export package.k --format=json-schema > inputs.schema.json
```

The exported JSON Schema carries `description` (from docstrings) and `x-ui` metadata (from decorators). Standard JSON-Schema-aware form renderers use it unchanged; akua-aware renderers pick up the ordering + grouping.

No `x-user-input` markers. No akua-specific JSON-Schema dialect. The `Input` schema **is** the customer-configurable contract by definition.

## See also

- [package-format.md](../../docs/package-format.md) — the canonical Package spec
- [cli.md `render` / `export`](../../docs/cli.md) — the two relevant verbs
- [02-webapp-postgres/](../02-webapp-postgres/) — next example: cross-source wiring
