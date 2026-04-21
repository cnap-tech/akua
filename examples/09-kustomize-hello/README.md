# 09-kustomize-hello

> **Status: temporarily broken.** The shell-out kustomize backend was
> removed in `e5b77dc` / Phase 0 of [`docs/roadmap.md`](../../docs/roadmap.md).
> `kustomize.build` now returns `E_ENGINE_NOT_AVAILABLE` until the
> embedded `kustomize-engine-wasm` lands (Phase 3). See
> [`docs/security-model.md`](../../docs/security-model.md) — akua
> doesn't shell out to external binaries in the render path, ever.
>
> This Package's source stays intact as the target shape for the embedded
> engine to satisfy. Rendering will resume once Phase 3 ships.

Smallest Package that exercises akua's `kustomize.build` engine
callable end-to-end.

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
