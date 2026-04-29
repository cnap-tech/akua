# Spike: `pkg.render` as `import upstream; upstream.render(input)`

**Status:** design accepted; implementation staged.

## Problem

Today's `pkg.render({ path = "./upstream", inputs = {...} })` violates
two emerging invariants:

1. **No filesystem paths in user-authored KCL** (CLAUDE.md). The
   `path = "./upstream"` literal is a path string in user code.
2. **Type-safety at the consumer site.** The call shape gives no
   guarantees that the inputs match the upstream's `Input` schema.

Goal: zero path strings, full type-safety on the input shape, and a
call site that reads naturally to a coding agent or human reviewer.

## Decision

Each Akua Package's `package.k` declares two akua-managed lines at
the bottom (templated by `akua init`, validated by `akua lint`):

```kcl
__id = "upstream"   # must equal [package].name in akua.toml
render = lambda input: Input -> [{str:}] { pkg.renderById(__id, input) }
```

Consumers reach the upstream via the natural module path:

```kcl
import upstream

_up = upstream.render(upstream.Input{ appName = "x" })
```

## Why this shape

- **No path strings in user code.** The two-line declaration captures
  the package's own canonical name (matches its `akua.toml`); the
  consumer references `upstream.render` by import alias.
- **Typed at every layer.** `upstream.Input` is type-checked by KCL
  at the consumer's call site. The lambda signature is type-checked
  in upstream's own source.
- **Author-visible, hookable.** The `render` lambda is the package's
  public API. Authors can add custom logic (logging, validation,
  caching, fan-out) before/after the `pkg.renderById` call. Same
  idiom as Python's `if __name__ == "__main__":`: small,
  conventional, hookable.
- **Reads naturally.** `upstream.render(input)` maps to every
  language's module-method pattern. An agent reads it correctly first
  try without docs.

## Why not the alternatives

### Codegen wrapper (rejected)

Synthesizing `_akua_render.k` next to `package.k` at materialization
time gives zero author burden, but path-deps register in-place at
`../upstream/`. Writing the shim into the user's source dirties their
tree; copying-into-cache breaks the dev-loop hot reload (edits to a
sibling Package no longer trigger re-render).

A KCL fork patch that injects a synthetic file at module-load time
would solve this but adds ~80–150 LoC of fork maintenance for one
templated line of API surface. Not worth it.

### `pkg.render(upstream.Input{...})` (rejected)

Pulling the package identity from the input schema's module of origin
would need a KCL fork patch to preserve `__schema_type__` in plugin
args. Even with that patch, the call site reads as "render an Input
value" — the action ("render upstream") is hidden behind type
inference. Coding agents would need to learn the magic from docs
rather than reading it from the call.

### Alias as a string at the call site (rejected)

`pkg.render({ package = "upstream", inputs = ... })` keeps the engine-
plugin shape (visible call) but reintroduces a stringly-typed package
reference. KCL doesn't catch typos; only `akua check` does. Type-
safety lives in the build tool, not the language. Worse ergonomics
than method-on-import for no architectural gain.

## Resolution mechanism

`pkg.renderById(id, input)` is the akua-internal plugin the lambda
wraps. It looks `id` up in the **current render frame's**
`resolved_deps` map (consumer's resolved deps), finds the path,
loads + renders the upstream package.

The map is keyed by the upstream's canonical `[package].name` (from
its `akua.toml`) — which is what `__id` captures in the lambda.
`akua lint` enforces `__id == [package].name`. `akua check` enforces
that every dep referenced via `import + .render(...)` is declared in
`[dependencies]`.

```
RenderFrame {
    package: PathBuf,                           // current Package being rendered
    allowed_roots: Vec<PathBuf>,                // existing
    strict: bool,                               // existing
    budget: BudgetSnapshot,                     // existing
    resolved_deps: HashMap<String, PathBuf>,    // NEW: canonical-name → path
}
```

When `outer.render()` runs:
1. Push frame for `outer` with `resolved_deps = outer's deps`.
2. KCL evaluates outer's body. Hits `inner.render(...)` — KCL invokes
   inner's render lambda (defined in inner's source).
3. Lambda body: `pkg.renderById(__id, input)`. `__id` is closed over
   = `"inner"` (inner's canonical name).
4. Plugin handler reads `current frame's resolved_deps["inner"]` →
   path. RenderScope's top is still `outer` because the lambda
   doesn't push a frame.
5. `PackageK::load(path).render(input)` runs — pushes inner's frame
   with inner's own resolved_deps. Inner's body evaluates. If inner
   calls `deep.render(...)`, the cycle repeats with inner's deps map.

## Implementation plan

Staged across PRs to keep each reviewable:

### Stage 1: Plumbing

- `RenderFrame` gains `resolved_deps: HashMap<String, PathBuf>`.
- `RenderScope::enter_with_deps(package, deps)` constructor.
- `kcl_plugin::resolve_dep(id) -> Option<PathBuf>` accessor.
- chart_resolver builds the canonical-name → path map and threads it
  through `render_in_worker` → render-worker → `RenderScope`.

### Stage 2: New plugin + stdlib

- `pkg.renderById(id, input)` host plugin in `pkg_render.rs`.
- `akua/pkg.k` stdlib exposes the typed call shape.
- Old `pkg.render({path, inputs})` plugin removed (no paths in KCL).

### Stage 3: Author tooling

- `akua init` template adds the two-line API surface at the bottom of
  the new `package.k`.
- `akua lint` rule:
  - `__id` declared and equals `[package].name`
  - `render` lambda declared with the canonical signature

### Stage 4: Migration + examples

- `examples/08-pkg-compose/` — migrate inner package to declare the
  render lambda; outer uses `inner.render(input)`.
- `examples/11-install-as-package/` — same migration.
- Goldens regenerated.

### Stage 5: Documentation

- `docs/package-format.md` documents the two-line API surface.
- `docs/cli.md` updates the `akua init` template.
- This spike doc moves to "implemented".

## Hookable extension point

The `render` lambda is a real hook. Examples authors might write:

```kcl
# Logging
render = lambda input: Input -> [{str:}] {
    _ = print("rendering ${__id} with ${input}")
    pkg.renderById(__id, input)
}

# Override behavior
render = lambda input: Input -> [{str:}] {
    pkg.renderById(__id, Input { ...input, replicas = max(input.replicas, 2) })
}

# Skip rendering under a flag
render = lambda input: Input -> [{str:}] {
    [] if input.skip else pkg.renderById(__id, input)
}
```

This is a strength, not friction. The lambda makes the author's
intent visible; if a Package wants to do something unusual at render
time, the hook is there.

## Open questions

- **Stricter typing on the lambda signature**: lint could parse the
  lambda and enforce `lambda input: Input -> [{str:}]` exactly. Or
  loose: any callable with the right arity. Pre-alpha can iterate.
- **`pkg.renderById` exposed via `akua.pkg`**: the consumer's
  `package.k` doesn't need to know about it (the lambda lives in
  upstream). But the upstream needs `import akua.pkg` to call it.
  Templated by `akua init`.
- **Backwards compatibility**: pre-alpha; we're free to drop the old
  `pkg.render({path})` plugin in one breaking change. Changelog entry
  + migration note.
