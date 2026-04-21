# helm-engine-wasm

Embedded Helm v4 template engine — a tiny Go program wrapping
`helm.sh/helm/v4/pkg/engine.Render`, compiled to `wasip1`, hosted via
[Extism](https://extism.org) from Rust.

Same plugin ABI family as Helm 4's HIP-0026. Lets `akua render` work
without a `helm` CLI on `$PATH`.

## Build

```bash
task build:helm-engine-wasm
```

Produces `assets/helm-engine.wasm` (~70 MB, ~12 MB gzipped). Embedded into
the akua binary via `include_bytes!`.

## Binary size

The wasm is large because Go's linker can't prune types exposed by a
package's public API. `pkg/engine.New(*rest.Config)` and
`pkg/chart/common.DefaultCapabilities = makeDefaultCapabilities()` (which
init-calls `k8s.io/client-go/kubernetes/scheme`) drag the entire k8s.io
dep tree (~267 packages) even though our call path never touches them.

### Option 2: fork + strip (future optimisation)

A future optimisation: vendor `pkg/engine/engine.go` + `funcs.go` +
`files.go` + a minimal `common.Capabilities` into this module, remove the
`rest.Config` import + `RenderWithClient*` functions + the
`k8s.io/client-go/kubernetes/scheme` init in capabilities. Expected size:
**~15 MB wasm (~3 MB gzipped)**, a 5× reduction. Cost: ~1500 LOC to
vendor, resync against upstream Helm tags every release (~quarterly).

Not done yet — the current bundled wasm works. Track as a size-optimisation
backlog item for when download size is user-complained.

## ABI

The plugin exports a single Extism function `render` that takes a JSON
input and returns a JSON output:

**Input**

```json
{
  "chart_tar_gz_b64": "<base64 of chart tarball>",
  "values_yaml": "<yaml string>",
  "release": { "name": "demo", "namespace": "default", "revision": 1 }
}
```

**Output**

```json
{
  "manifests": { "mychart/templates/cm.yaml": "<rendered yaml>" },
  "error": ""
}
```

## Sandbox

Extism's defaults deny filesystem, network, env, and exec. This plugin
never does I/O — the Rust host pre-fetches any chart deps into
`charts/` before tarring the chart for this plugin.
