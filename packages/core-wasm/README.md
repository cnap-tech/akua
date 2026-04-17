# @akua/core-wasm

WebAssembly bindings for [`akua-core`](../../crates/akua-core), consumable from
Node.js and browser bundlers.

## Exported functions

- `hashToSuffix(input, length)` — djb2 → base36, deterministic chart aliases
- `extractInstallFields(schema)` — walk a JSON Schema, return
  `x-user-input` / `x-install` fields
- `applyInstallTransforms(fields, inputs)` — slugify + template (`{{value}}`)
  over `{path: string}` inputs, returns resolved nested values
- `validateValuesSchema(schema)` — structural validation, returns error
  message or `null`
- `mergeHelmSourceValues(sources)` — deep-merge values from multiple sources,
  nested by alias
- `buildUmbrellaChart(name, version, sources)` — assemble umbrella Chart.yaml
  + merged values + git-source passthrough

All functions share the exact same implementation as the native Rust core
and the CLI — no TS reimplementation drift.

## Build

This package is generated. To rebuild:

```bash
pnpm wasm:build          # from repo root
# or directly:
wasm-pack build crates/akua-wasm --target bundler --out-dir ../../packages/core-wasm
```

For Node consumption, use `--target nodejs` instead; for a plain `.wasm` file,
`--target web`.

## Smoke test

```bash
wasm-pack build crates/akua-wasm --target nodejs --out-dir ../../packages/core-wasm
node packages/core-wasm/smoke-test.mjs
```
