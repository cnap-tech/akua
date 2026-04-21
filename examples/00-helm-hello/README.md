# 00-helm-hello

The smallest Package that exercises akua's `helm.template` engine
callable end-to-end. Unlike 01–07, **this one renders with the
shipping binary** — provided the `engine-helm-shell` feature is on
and `helm` is on PATH.

## What's here

| file | purpose |
|---|---|
| `package.k` | KCL Package; imports `akua.helm`, calls `helm.template`, wires the result into `resources = …`. |
| `akua.toml` | Manifest — no external deps. |
| `inputs.example.yaml` | Auto-discovered by `akua render` when `--inputs` is omitted. |
| `chart/` | A tiny in-tree Helm chart (one `ConfigMap` template). |

## Render

`package.k` passes `"./chart"` to `helm.template`; akua resolves that
against the Package.k's directory, so `akua render` works from any
cwd — point `--package` at this directory:

```sh
cargo run -q --features engine-helm-shell -p akua-cli -- \
    render --package examples/00-helm-hello/package.k --out ./rendered
```

Or from inside this directory:

```sh
cargo run -q --features engine-helm-shell -p akua-cli -- render --out ./rendered
```

Once a release binary ships with the feature built in:

```sh
akua render --package examples/00-helm-hello/package.k --out ./rendered
```

The rendered `ConfigMap` lands at `./rendered/000-configmap-hello-greeting.yaml`
— already checked in alongside the example so you can eyeball the
output without running anything.

## What's happening

`package.k` imports `akua.helm` — the bundled akua KCL stdlib, a
thin typed wrapper over `kcl_plugin.helm` — and calls
`helm.template(chart_path, values, release_name, release_namespace)`.
Under the hood, akua's plugin dispatcher routes the call to a Rust
handler that shells out to `helm template`, parses the multi-document
YAML output back into KCL values, and splats them into `resources`.

The shell-out engine is transitional. A future WASM-embedded
`helm-engine` will register under the same `helm.template` name; the
Package authoring surface won't change.

## Spec

See [`docs/package-format.md §5`](../../docs/package-format.md#5-outputs--what-akua-emits)
for the `outputs` shape and [`docs/cli.md` `akua render`](../../docs/cli.md#akua-render).
