# helm v4 fork — strip client-go

Patch set against `helm.sh/helm/v4@v4.1.4` that removes the transitive
`k8s.io/client-go` dependency from the Go→wasip1 Helm engine.

## Why

Upstream helm drags `client-go` into the binary via a deliberate
blank-import (`internal/version/clientgo.go`) so that
`debug.BuildInfo` can report the client-go module version. That
blank-import pulls **~60 MB** of k8s tooling that akua never uses —
no cluster, no kubectl, no dynamic clients. The renderer is offline.

## Measurement

Benchmark on `mychart` (3 templates, 1 subchart) — macOS arm64:

| | upstream v4.1.4 | fork |
|---|---|---|
| `helm-engine.wasm` | 75 MB | **20 MB** (-73%) |
| `akua` binary | ~90 MB | **44.6 MB** (-50%) |
| `akua render` wall | ~7s | **~2.3s** (-67%) |
| Cranelift CPU burn | ~32s | ~9s (-72%) |
| client-go deps | 22 | **0** |

Combined with build-time precompile (see `perf/wasm-precompile`):
projected ~75 MB binary, sub-500ms render, no Cranelift at runtime.

## What the patches do

1. **`pkg/engine/engine.go`** — drop `New(*rest.Config)` and
   `RenderWithClient(..., *rest.Config)`. Keep `Render` and
   `RenderWithClientProvider` (provider-agnostic).
2. **`pkg/engine/lookup_func.go`** — remove client-go /
   dynamic / discovery imports. `ClientProvider` interface kept
   (API compat) but returns `any`. `NewLookupFunction` stub returns
   an error at render time. Chart templates that call `lookup`
   will error — a deliberate regression; akua does not speak to
   live clusters.
3. **`pkg/chart/common/capabilities.go`** — replace the
   `client-go/kubernetes/scheme` walk with a hand-maintained
   `VersionSet` of common API groups. Replace
   `helmversion.K8sIOClientGoModVersion()` with a hardcoded
   kube version string derived from the upstream `go.mod`
   client-go version.
4. **`internal/version/clientgo.go`** — remove the blank-import
   that was forcing client-go into the build. Stub
   `K8sIOClientGoModVersion()` since nothing calls it anymore.

## How to apply

```sh
cd $(mktemp -d)
git clone --depth 1 --branch v4.1.4 https://github.com/helm/helm.git helm-v4
cp -r helm-v4 helm-fork
cd helm-fork
patch -p1 < /path/to/akua/crates/helm-engine-wasm/fork/helm-v4.1.4.patch
```

Then point `crates/helm-engine-wasm/go-src/go.mod` at the fork:

```
replace helm.sh/helm/v4 => /path/to/helm-fork
```

And rebuild:

```sh
cd crates/helm-engine-wasm/go-src
GOOS=wasip1 GOARCH=wasm go build -buildmode=c-shared \
  -o ../assets/helm-engine.wasm -ldflags='-s -w' .
```

## Maintenance

The fork is tracked against upstream v4.1.4. When bumping helm:

1. Clone the new upstream tag.
2. Apply this patch — expect conflicts in `engine.go` (most
   likely) and `capabilities.go` (common-spot for upstream churn).
3. Resolve, re-run the akua integration tests.
4. Refresh this patch file.

Surface area is small (~100 lines), so upstream sync should take
~30 min per release.

## Productionisation options

- **Vendor the fork** in-tree under `crates/helm-engine-wasm/third_party/helm-v4-fork/`.
  Simplest, largest diff (~300 MB checked in).
- **Submodule** pointing at `cnap-tech/helm-v4-fork` (a mirror of
  `helm/helm` with this patch landed on a `akua-wasip1` branch).
  Cleanest, requires maintaining a public mirror.
- **Patch-at-build-time** — build.rs clones upstream, applies
  the patch, builds the WASM. Requires `git` + `go` in the build
  environment. Brittle but requires no ongoing maintenance beyond
  the patch file.
