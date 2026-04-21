# 08-pkg-compose

Package composition via `pkg.render` — an outer Package calls a
reusable inner Package twice with different inputs and concatenates
the results. Renders end-to-end today (pure KCL; no helm needed).

## What's here

| file | purpose |
|---|---|
| `package.k` | Outer Package; calls `pkg.render("./shared", ...)` twice with distinct inputs. |
| `shared/package.k` | Inner Package; emits one ConfigMap parameterized by `name` + `payload`. |
| `akua.toml` | Outer manifest — no external deps. |
| `inputs.example.yaml` | Per-component inputs, auto-discovered by `akua render`. |

## Render

```sh
cargo run -q -p akua-cli -- render --package examples/08-pkg-compose/package.k --out ./rendered
```

Or, from the example directory:

```sh
akua render --out ./rendered
```

Two ConfigMaps land in `./rendered/` (checked in as reference output):

```
rendered/
├── 000-configmap-frontend.yaml
└── 001-configmap-backend.yaml
```

## How `pkg.render` works

KCL's upstream plugin mechanism holds a global mutex across every
plugin invocation, which prevents same-thread re-entry into the
evaluator. Calling `PackageK::render` recursively inside the plugin
handler would deadlock.

Instead, the `pkg.render` handler is **cheap**: it returns a
sentinel dict of shape

```json
{ "akuaPkgRenderSentinel": { "path": "…", "inputs": {…} } }
```

placed in the caller's `resources` list. After the outer Package's
`eval_kcl` completes (and KCL's mutex is released), akua-core walks
the resources list, finds the sentinels, loads + renders the
referenced Packages, and splices each nested resource list in
place. Cycle detection uses akua-core's thread-local render-stack —
a Package referring to itself (direct or transitive) is rejected
before infinite recursion.

Nested `pkg.render` calls — e.g. `A` uses `B`, which uses `C` —
expand recursively via the same walk; each inner Package's own
`render()` walks its own sentinels.
