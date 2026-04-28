# @akua-dev/sdk examples

Runnable recipes showing each of the SDK's verbs. Every file is a
standalone `bun run <file>` target — no configuration, no CLI binary
required (except the two recipes that exercise shell-out verbs, noted
inline).

Run any recipe from this directory:

```sh
bun run 01-render-source.ts
```

Or from the repo root:

```sh
bun run packages/sdk/examples/01-render-source.ts
```

## Recipes

| # | file | verb | transport | binary? |
|---|---|---|---|---|
| 01 | [`01-render-source.ts`](01-render-source.ts) | `renderSource` | WASM | no |
| 02 | [`02-lint-package.ts`](02-lint-package.ts) | `lint` | WASM | no |
| 03 | [`03-inspect-options.ts`](03-inspect-options.ts) | `inspect` | WASM | no |
| 04 | [`04-check-workspace.ts`](04-check-workspace.ts) | `check` | WASM | no |
| 05 | [`05-tree-deps.ts`](05-tree-deps.ts) | `tree` | WASM | no |
| 06 | [`06-diff-renders.ts`](06-diff-renders.ts) | `diff` | WASM | no |
| 07 | [`07-fmt-in-place.ts`](07-fmt-in-place.ts) | `fmt` | WASM | no |
| 08 | [`08-shell-out-render.ts`](08-shell-out-render.ts) | `render` | shell-out | **yes** |

## The pattern

Every method on `Akua` returns a typed result validated against a
JSON Schema generated from the Rust source. Contract drift throws
at the parse boundary:

```ts
try {
  const out = await akua.check({ workspace: './my-pkg' });
  // out.status: "ok" | "fail"
  // out.checks: CheckResult[]
} catch (err) {
  if (err instanceof AkuaContractError) {
    // stdout didn't match the schema — the bundle's ahead of the SDK
  }
}
```

See [`../README.md`](../README.md) for the per-verb transport
breakdown and [`docs/sdk.md`](../../../docs/sdk.md) for the full
spec.
