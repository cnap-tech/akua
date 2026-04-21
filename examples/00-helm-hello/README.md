# 00-helm-hello

The smallest Package that exercises akua's `helm.template` engine
callable end-to-end. Unlike 01–07, **this one renders with the
shipping binary** — provided the `engine-helm-shell` feature is on
and `helm` is on PATH.

## What's here

| file | purpose |
|---|---|
| `package.k` | KCL Package; imports `kcl_plugin.helm`, calls `helm.template`, wires the result into `resources = …`. |
| `akua.toml` | Manifest — no external deps. |
| `inputs.example.yaml` | Auto-discovered by `akua render` when `--inputs` is omitted. |
| `chart/` | A tiny in-tree Helm chart (one `ConfigMap` template). |

## Render

> **Relative chart path.** `package.k` passes `"./chart"` to
> `helm.template`, which the current shell-out engine resolves
> against the **process cwd** — run from this directory, not the
> workspace root. A follow-up will anchor relative chart paths to
> the Package.k's own location.

From this directory:

```sh
cargo run -q --features engine-helm-shell -p akua-cli -- render --out ./deploy
```

Or, once a release binary ships with the feature built in:

```sh
akua render --out ./deploy
```

The rendered `ConfigMap` lands at `./deploy/000-configmap-hello-greeting.yaml`.

## What's happening

`package.k` imports `kcl_plugin.helm` — KCL's plugin namespace — and
calls `helm.template(chart_path, values, release_name, release_namespace)`.
Under the hood, akua's plugin dispatcher routes the call to a Rust
handler that shells out to `helm template`, parses the multi-document
YAML output back into KCL values, and splats them into `resources`.

The shell-out engine is transitional. A future WASM-embedded
`helm-engine` will register under the same `helm.template` name; the
Package authoring surface won't change.

## Spec

See [`docs/package-format.md §5`](../../docs/package-format.md#5-outputs--what-akua-emits)
for the `outputs` shape and [`docs/cli.md` `akua render`](../../docs/cli.md#akua-render).
