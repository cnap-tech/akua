# helmfile-package

Wraps a [helmfile](https://github.com/helmfile/helmfile) as an Akua source.
The engine runs `helmfile template`, pre-renders all releases to YAML, and
ships the result as a single static subchart inside the umbrella.

## Prerequisites

`helmfile` and `helm` on `$PATH` (pin via `mise install`).

## Flow

```bash
cargo run -p akua-cli -- build --package examples/helmfile-package --out dist/helmfile-chart
```

What Akua does:

1. Resolves `engine: helmfile` → `HelmfileEngine`.
2. Runs `helmfile --file ./helmfile.yaml template`.
3. Writes a subchart with the captured YAML as `templates/rendered.yaml`.
4. Adds a `file://` dep to the umbrella Chart.yaml.

## Migration story

Existing helmfile users can wrap their current `helmfile.yaml` with a
four-line `package.yaml` and get:

- OCI artifact output (content-addressable, immutable)
- Schema-validated customer inputs via `x-user-input`
- Browser-side live preview via WASM bindings
- `.akua/metadata.yaml` provenance

No rewrite required.

## Determinism caveat

Helmfile supports `now`, `exec`, env reads — these break content-addressing.
If you care about reproducible OCI digests, avoid those template functions
or the same inputs will produce different chart digests across builds.
