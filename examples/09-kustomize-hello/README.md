# 09-kustomize-hello

> **Renders end-to-end** via the embedded `kustomize-engine-wasm`. No
> `kustomize` binary on `$PATH` needed or consulted. Kustomize runs
> inside a wasmtime WASI sandbox against an in-memory filesystem
> unpacked from a tar.gz sent over the WASM ABI. See
> [`docs/security-model.md`](../../docs/security-model.md) +
> [`docs/roadmap.md`](../../docs/roadmap.md) Phase 3.

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
task build:kustomize-engine-wasm          # once per machine
akua render --package examples/09-kustomize-hello/package.k --out ./rendered
```

The rendered `ConfigMap` lands at
`./rendered/000-configmap-prod-hello.yaml` — named `prod-hello` with
the overlay's `env: prod` label applied. Checked in alongside the
example so you can eyeball the output without running anything.

## Spec

See [`docs/package-format.md`](../../docs/package-format.md) for the
Package shape and [`docs/cli.md` `akua render`](../../docs/cli.md#akua-render).
