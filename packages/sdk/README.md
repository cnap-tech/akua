# @akua/sdk

TypeScript SDK for [Akua](https://github.com/cnap-tech/akua). Wraps the
`akua-core` Rust library via WebAssembly so you can assemble umbrella
charts, extract install fields, and evaluate CEL transforms from Node
or the browser **without installing the `akua` CLI**.

## Install

Published on [JSR](https://jsr.io/@akua/sdk) — works in Deno, Node,
Bun, and any bundler via JSR's npm-compat layer.

```bash
# Deno
deno add jsr:@akua/sdk

# Node / bun / pnpm / npm (JSR's npm-compat shim)
npx jsr add @akua/sdk

# Browser entry (same package, different export)
# import { init, ... } from '@akua/sdk/browser';
```

## Quick start

```ts
import {
  init,
  buildUmbrellaChart,
  extractInstallFields,
  applyInstallTransforms,
} from '@akua/sdk';

// One-time init per process / page. Cheap in Node, lazy-instantiates
// the .wasm in the browser.
await init();

// 1. Author-time: assemble an umbrella chart from multiple sources.
const umbrella = buildUmbrellaChart('hello', '0.1.0', [
  {
    name: 'app',
    helm: {
      repo: 'https://charts.bitnami.com/bitnami',
      chart: 'nginx',
      version: '18.1.0',
    },
    values: { replicaCount: 1 },
  },
]);
// → { chartYaml: { apiVersion: 'v2', name: 'hello', … }, values: { app: { … } } }

// 2. Install-time: extract user-input fields from a schema and apply
//    CEL transforms over user-provided values.
const schema = {
  type: 'object',
  properties: {
    httpRoute: {
      type: 'object',
      properties: {
        hostname: {
          type: 'string',
          'x-user-input': { order: 10 },
          'x-input': { cel: "slugify(value) + '.apps.example.com'" },
        },
      },
      required: ['hostname'],
    },
  },
  required: ['httpRoute'],
};

const fields = extractInstallFields(schema);
const resolved = applyInstallTransforms(fields, {
  'httpRoute.hostname': 'My App!',
});
// → { httpRoute: { hostname: 'my-app.apps.example.com' } }
```

## What's exposed

| Function | Purpose |
|---|---|
| `init()` | One-shot setup. Loads the WASM payload. Safe to call multiple times. |
| `buildUmbrellaChart(name, version, sources)` | Assemble a Helm v2 `Chart.yaml` + merged `values.yaml`. |
| `extractInstallFields(schema)` | Walk a JSON Schema, return `x-user-input` fields. |
| `applyInstallTransforms(fields, inputs)` | Evaluate `x-input.cel` against raw user inputs. |
| `validateValuesSchema(schema)` | Structural validation. Returns `null` when valid. |
| `mergeSourceValues(sources)` | Deep-merge the `values` blocks from multiple sources. |
| `mergeValuesSchemas(sources)` | Combine per-source schemas into one umbrella schema. |
| `hashToSuffix(input, length)` | djb2 + base36 short suffix. Rarely needed directly. |

## Runtime targets

The package ships both `wasm-pack` build targets:

- **Node** — loaded via `import 'node'` conditional export. Sync instantiation, no fetch.
- **Browser / bundler** — loaded via `default` export. Vite, webpack, esbuild, Bun, and Deno all know how to bundle the `.wasm` artifact.

Pick via `package.json`'s `exports` map; you don't import from a specific subpath.

## Not included (use the CLI for these)

- `akua render` — shells out to the embedded Helm engine; needs a filesystem and isn't WASM-safe today.
- `akua build` + `akua package` + `akua publish` — touch disk / network; wrap via the CLI.
- `akua inspect oci://<ref>` — OCI pull needs the native TLS stack.

For those, install the CLI (`brew install akua` / prebuilt binaries on GitHub Releases).

## Versioning

Tracks `apiVersion: akua.dev/v1alpha1` for the manifest shape. Function
signatures listed above are frozen for the `v1alpha1` lifetime.

## License

Apache-2.0
