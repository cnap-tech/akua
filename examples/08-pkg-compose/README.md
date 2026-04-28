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

`pkg.render` is a synchronous KCL host plugin: the handler resolves
the path against the calling Package's directory (with the same
sandbox guard `helm.template` and `kustomize.build` use), calls
`PackageK::load(...).render(inputs)` inline, and returns the inner
resources list directly to the caller. Because the return is a real
list, list-comprehension patches and filter expressions on the
result work natively — no post-eval rewrite step.

Reentrancy works because akua's KCL fork copies the plugin handler
fn pointer out of its mutex before invoking the callback (see
`cnap-tech/kcl#akua-wasm32`); upstream KCL holds the mutex across
the call and would deadlock here.

Cycle detection uses akua-core's thread-local render-stack: a
Package referring to itself, directly or transitively (`A → B → A`),
is rejected before infinite recursion. Nested `pkg.render` calls
(`A → B → C`) recurse through the same handler, with each inner
Package's render pushing/popping the stack.
