# kcl-package

Demonstrates authoring a package in [KCL](https://kcl-lang.io/). The `kcl`
engine compiles `app.k` into Kubernetes YAML at build time and wraps it as a
static subchart inside the umbrella.

## Prerequisites

None. KCL's evaluator is linked in-process as a native Rust crate —
no `kcl` CLI required.

## Flow

```bash
# From the repo root:
cargo run -p akua-cli -- build --package examples/kcl-package --out dist/kcl-chart
```

What Akua does:

1. Resolves `engine: kcl` → `KclEngine` (feature-gated, on by default).
2. Calls `kcl_lang::API::exec_program` on the `.k` source.
3. Writes a subchart under `dist/kcl-chart/<source-id>/`:
   - `Chart.yaml` (generated)
   - `templates/rendered.yaml` (the KCL output)
4. Adds a `file://...` dependency to the umbrella `Chart.yaml`.

First build takes a few minutes (KCL's dep graph is heavy); subsequent
builds in the same workspace are fast.

## Browser note

Native Rust KCL can't cross-compile to wasm32, so `akua-wasm` (the
browser bindings) doesn't compile in a KCL engine. Browser apps that
need to render KCL source use
[`@kcl-lang/wasm-lib`](https://www.npmjs.com/package/@kcl-lang/wasm-lib)
directly (official upstream package wrapping `kcl.wasm`) and pass the
rendered YAML into akua-wasm's umbrella assembler separately.

The output chart is 100% vanilla Helm. Deploy with `helm install` or
ArgoCD against the OCI digest once published.

## When to use the KCL engine

- You prefer a typed config language over Helm templates.
- Early binding is fine — customers don't `--set` values at deploy time;
  their inputs are baked in at build time.
- You want Akua's schema + CEL + provenance layer without rewriting
  existing KCL code.
