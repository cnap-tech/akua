# 00-helm-hello

> **Renders end-to-end** via the embedded `helm-engine-wasm`. No `helm`
> binary on `$PATH` needed or consulted. All template rendering happens
> inside a wasmtime WASI sandbox. See
> [`docs/security-model.md`](../../docs/security-model.md) +
> [`docs/roadmap.md`](../../docs/roadmap.md) Phase 1.

The smallest Package that exercises akua's `helm.template` engine
callable end-to-end.

## What's here

| file | purpose |
|---|---|
| `package.k` | KCL Package; imports `akua.helm`, calls `helm.template`, wires the result into `resources = …`. |
| `akua.toml` | Manifest — no external deps. |
| `inputs.example.yaml` | Auto-discovered by `akua render` when `--inputs` is omitted. |
| `chart/` | A tiny in-tree Helm chart (one `ConfigMap` template). |

## Render

`package.k` passes `"./chart"` to `helm.template`; akua resolves that
against the Package.k's directory (via the path-traversal-guarded
`resolve_in_package`) and hands the chart tarball to the embedded
WASM Helm engine. `akua render` works from any cwd — point
`--package` at this directory:

```sh
# Build the embedded helm engine once per machine:
task build:helm-engine-wasm

akua render --package examples/00-helm-hello/package.k --out ./rendered
```

The rendered `ConfigMap` lands at `./rendered/000-configmap-hello-greeting.yaml`
— already checked in alongside the example so you can eyeball the
output without running anything.

## What's happening

`package.k` imports `akua.helm` — the bundled akua KCL stdlib, a
thin typed wrapper over `kcl_plugin.helm` — and calls
`helm.template(helm.Template { ... })`. Under the hood, akua's plugin
dispatcher routes the call to a Rust handler that tars the chart
directory, hands it to a **Go program compiled to wasm32-wasip1**
hosted via wasmtime (see [`crates/helm-engine-wasm/`](../../crates/helm-engine-wasm/)),
parses the multi-document YAML output back into resources, and
splats them into `resources`.

No `helm` binary touched. No subprocess. No `$PATH`. The entire
render path lives inside a WASI sandbox. Per CLAUDE.md: "No
shell-out, ever."

## Spec

See [`docs/package-format.md §5`](../../docs/package-format.md#5-outputs--what-akua-emits)
for the `outputs` shape and [`docs/cli.md` `akua render`](../../docs/cli.md#akua-render).
