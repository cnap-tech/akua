# 09-kustomize-hello

Smallest Package that exercises akua's `kustomize.build` engine
callable end-to-end. Like 00 and 08, **this one renders with the
shipping binary** — provided the `engine-kustomize-shell` feature is
on and `kustomize` is on PATH.

## What's here

| file | purpose |
|---|---|
| `package.k` | KCL Package; imports `akua.kustomize`, calls `kustomize.build("./overlay")`, wires the result into `resources`. |
| `akua.toml` | Manifest — no external deps. |
| `base/` | Base layer — a single `ConfigMap`. |
| `overlay/` | Overlay — adds a `namePrefix` + `commonLabels`. |

## Render

```sh
cargo run -q --features engine-kustomize-shell -p akua-cli -- \
    render --package examples/09-kustomize-hello/package.k --out ./rendered
```

The rendered `ConfigMap` lands at
`./rendered/000-configmap-prod-hello.yaml` — named `prod-hello` with
the overlay's `env: prod` label applied. Checked in alongside the
example so you can eyeball the output without running anything.

## Spec

See [`docs/package-format.md`](../../docs/package-format.md) for the
Package shape and [`docs/cli.md` `akua render`](../../docs/cli.md#akua-render).
