# @akua/sdk

TypeScript SDK for [Akua](https://github.com/cnap-tech/akua). Build,
inspect, pack, and pull Helm charts from Node or the browser — no
`helm` or `akua` binary required. Pure Rust core, delivered via WASM.

## Install

Published on [JSR](https://jsr.io/@akua/sdk). Works in Deno, Node, Bun,
and any bundler via JSR's npm-compat layer.

```bash
deno add jsr:@akua/sdk        # Deno
npx jsr add @akua/sdk         # Node / Bun / pnpm / npm
```

Browser entry is a separate sub-export (`@akua/sdk/browser`); it
excludes Node-only helpers (`dockerConfigAuth`, etc.).

## 30-second tour

```ts
import { init, pullChart, inspectChartBytes, buildUmbrellaChart, packChart } from '@akua/sdk';

await init();  // One-shot. Cheap in Node, lazy-instantiates the .wasm in the browser.

// Pull a chart — dispatches on scheme: oci:// or https://.
const bytes = await pullChart('oci://ghcr.io/stefanprodan/charts/podinfo:6.7.1');

// Inspect without writing to disk.
const info = await inspectChartBytes(bytes);
console.log(info.chartYaml.name, info.chartYaml.version);

// Or compose an umbrella chart from multiple sources and pack a .tgz.
const umbrella = buildUmbrellaChart('my-pkg', '0.1.0', [
  { name: 'app', helm: { repo: 'oci://ghcr.io/you', chart: 'app', version: '1.0.0' } },
]);
const subcharts = new Map([['app', bytes]]);
const tgz = await packChart(umbrella, subcharts);
// → deploy via ArgoCD / Flux / `helm install ./pkg.tgz`.
```

## Pulling charts

`pullChart(ref, options)` returns the raw `.tgz` bytes. It dispatches on
scheme — one entry point for both OCI and classic Helm HTTP repos.

```ts
await pullChart('oci://ghcr.io/stefanprodan/charts/podinfo:6.7.1');
await pullChart('https://charts.jetstack.io/cert-manager:v1.16.1');
```

**Private registries.** Pass per-host credentials via `auth`:

```ts
await pullChart('oci://ghcr.io/you/private-chart:1.0.0', {
  auth: {
    'ghcr.io': { token: process.env.GHCR_PAT! },
  },
});
```

Credential shapes by registry:

| Registry | Credentials |
|---|---|
| GHCR private | `{ token: '<PAT with read:packages>' }` |
| Harbor | `{ username, password: '<robot-token>' }` |
| Private ECR | `{ username: 'AWS', password: '<aws ecr get-login-password>' }` |
| Private GAR | `{ username: '_json_key', password: '<service-account JSON>' }` |
| Docker Hub private | `{ username, password: '<access token>' }` |

**Reuse `~/.docker/config.json`** (Node only):

```ts
import { pullChart, dockerConfigAuth } from '@akua/sdk';
await pullChart('oci://…', { auth: await dockerConfigAuth() });
```

Handles the standard `auths`, `identitytoken`, `credHelpers`, and
`credsStore` entries (spawning `docker-credential-*` helpers as needed).

**Safety limits.** `maxBytes` (default 100 MB) caps the downloaded
layer; `AbortSignal` cancels in-flight fetches.

```ts
await pullChart('oci://…', {
  maxBytes: 20 * 1024 * 1024,
  signal: AbortSignal.timeout(30_000),
});
```

## Inspecting charts

```ts
import { inspectChartBytes } from '@akua/sdk';
const info = await inspectChartBytes(bytes);
// { chartYaml, valuesYaml?, valuesSchema?, akuaMetadata? }
```

Takes raw `.tgz` bytes (`Uint8Array | ArrayBuffer | Blob | ReadableStream`).
Streaming variant available via `streamTgzEntries` if you want per-entry
control.

## Composing umbrella charts

```ts
import { buildUmbrellaChart, packChart, buildMetadata, mergeValuesSchemas } from '@akua/sdk';

const sources = [
  { name: 'app', helm: { repo: 'oci://ghcr.io/you', chart: 'app', version: '1.0.0' } },
  { name: 'db', helm: { repo: 'https://charts.bitnami.com/bitnami', chart: 'postgresql', version: '15.0.0' } },
];

// 1. Build the umbrella Chart.yaml + merged values.yaml.
const umbrella = buildUmbrellaChart('my-pkg', '0.1.0', sources);

// 2. Pull each subchart. Dedupe identical (repo, name, version) triples.
const subcharts = new Map<string, Uint8Array>();
for (const dep of umbrella.chartYaml.dependencies ?? []) {
  const key = dep.alias ?? dep.name;
  subcharts.set(key, await pullChart(`${dep.repository}/${dep.name}:${dep.version}`));
}

// 3. Optionally produce a merged JSON Schema for install wizards.
const mergedSchema = mergeValuesSchemas([
  { source: sources[0], schema: await loadSchema('app') },
]);

// 4. Pack it all. Scrubs file:// repos. Emits .akua/metadata.yaml and
//    values.schema.json when provided.
const tgz = await packChart(umbrella, subcharts, {
  valuesSchema: mergedSchema,
  metadata: buildMetadata(sources),  // honours SOURCE_DATE_EPOCH
  signal,
});
```

### `dependencyToOciRef`

Helper that joins `repository/name:version` for OCI deps, `null`
otherwise:

```ts
import { dependencyToOciRef } from '@akua/sdk';
const ref = dependencyToOciRef(dep);  // 'oci://ghcr.io/you/app:1.0.0' | null
```

## Install-wizard primitives

The "customer inputs" side of things — same primitives the CLI uses.

```ts
import { extractInstallFields, applyInstallTransforms, validateValuesSchema } from '@akua/sdk';

const schema = {
  type: 'object',
  properties: {
    hostname: {
      type: 'string',
      'x-user-input': { order: 10 },
      'x-input': { cel: "slugify(value) + '.apps.example.com'" },
    },
  },
  required: ['hostname'],
};

if (validateValuesSchema(schema)) throw new Error('bad schema');

const fields = extractInstallFields(schema);
// → [{ path: 'hostname', schema: {...}, required: true }]

const resolved = applyInstallTransforms(fields, { hostname: 'My App!' });
// → { hostname: 'my-app.apps.example.com' }
```

## Error handling

All SDK-thrown errors extend `AkuaError`:

```ts
import { AkuaError, OciPullError, HelmHttpError, TarError, WasmInitError } from '@akua/sdk';

try {
  await pullChart(ref);
} catch (err) {
  if (err instanceof AkuaError) {
    // One of: OciPullError, HelmHttpError, TarError, WasmInitError, DockerConfigError.
    console.error(err.name, err.message);
  } else if (err instanceof DOMException && err.name === 'AbortError') {
    // Cancellation — propagate.
    throw err;
  } else {
    // Native fetch / network errors pass through unwrapped. Treat as transport.
  }
}
```

## API surface

### Chart I/O

| Function | Purpose |
|---|---|
| `pullChart(ref, options?)` | Pull a chart `.tgz`. Supports `oci://` and `https://`/`http://`. |
| `pullChartStream(ref, options?)` | Streaming variant — returns `ReadableStream<Uint8Array>`. |
| `pullHelmHttpChart(ref, options?)` | Just the HTTP Helm repo path. `pullChart` calls this. |
| `pullChartCached(ref, options?)` | **`@akua/sdk/cache`** — Node-only on-disk cache. |
| `inspectChartBytes(tgz)` | Parse a chart tarball — returns `Chart.yaml`, `values.yaml`, schema, metadata. |
| `packChart(umbrella, subcharts, options?)` | Serialize to Helm-compatible `.tgz`. |
| `packChartStream(umbrella, subcharts, options?)` | Streaming variant — pipe to fetch body / disk. |

### Composition

| Function | Purpose |
|---|---|
| `buildUmbrellaChart(name, version, sources)` | Helm v2 `Chart.yaml` + merged `values.yaml`. |
| `mergeSourceValues(sources)` | Deep-merge values blocks, nested by alias. |
| `mergeValuesSchemas(sources)` | Combine per-source schemas into one umbrella schema. |
| `dependencyToOciRef(dep)` | Canonical OCI ref for a `ChartDependency`, or `null`. |
| `buildMetadata(sources, fields?, options?)` | Produces `.akua/metadata.yaml` provenance. |

### Install-time

| Function | Purpose |
|---|---|
| `extractInstallFields(schema)` | Walk JSON Schema, return `x-user-input` leaves. |
| `applyInstallTransforms(fields, inputs)` | Evaluate `x-input.cel` against user inputs. |
| `validateValuesSchema(schema)` | Structural schema validation. `null` when OK. |

### Auth (Node only)

| Function | Purpose |
|---|---|
| `dockerConfigAuth(options?)` | Read `~/.docker/config.json` → `OciAuth`. |

### Primitives

| Function | Purpose |
|---|---|
| `hashToSuffix(input, length)` | djb2 + base36 short alias suffix. |
| `streamTgzEntries(tgz)` | Async-iterator over tar entries (low-level). |
| `packTgzStream(chartName, entries)` | Low-level tar+gzip writer. |
| `unpackTgz(tgz)` | Buffered tar reader (prefer `streamTgzEntries`). |
| `parseOciRef(ref)`, `parseHelmHttpRef(ref)` | Exported for tests; callers use `pullChart`. |
| `findIndexEntry(indexYaml, chart, version)` | Helm repo `index.yaml` lookup. |

## Runtime targets

The package ships two `wasm-pack` build targets:

- **Node entry** (`@akua/sdk`) — sync WASM instantiation, includes
  `dockerConfigAuth` and other Node-only helpers.
- **Browser entry** (`@akua/sdk/browser`) — bundler-compatible WASM load,
  no `node:fs` / `node:child_process` imports.

## Streaming pulls

For very large charts, or when piping straight into
`inspectChartBytes` / Convex uploads, use `pullChartStream`:

```ts
import { pullChartStream, inspectChartBytes } from '@akua/sdk';

const stream = await pullChartStream('oci://…:1.0.0');
const info = await inspectChartBytes(stream);
// Chart is never fully buffered in memory.
```

## On-disk cache (`@akua/sdk/cache`, Node only)

A Node-only wrapper that shares the CLI's `$XDG_CACHE_HOME/akua/v1/`
layout. One import, transparent cache hits, byte-identical to the
CLI's tarballs:

```ts
import { init } from '@akua/sdk';
import { pullChartCached } from '@akua/sdk/cache';

await init();
const bytes = await pullChartCached('oci://ghcr.io/…:1.0.0');
// First call: network pull + cache write. Next call: zero-network.
```

Respects `AKUA_NO_CACHE=1` and `AKUA_CACHE_DIR`. CLI and SDK hits
interop — warming the cache via `akua inspect` speeds up subsequent
SDK calls (and vice versa).

## Safety limits

Every I/O path in the SDK has hard caps to protect against malicious
inputs:

| Limit | Default | Path |
|---|---|---|
| Max download bytes (`maxBytes` option) | 100 MB | `pullChart` / `pullHelmHttpChart` |
| Max extracted bytes (`maxTotalBytes`) | 500 MB | `streamTgzEntries` / `unpackTgz` |
| Max tar entries (`maxEntries`) | 20 000 | `streamTgzEntries` / `unpackTgz` |
| Max single-entry bytes (`maxEntryBytes`) | 100 MB | `streamTgzEntries` / `unpackTgz` |
| SSRF host allowlist | on | all fetch paths |

SSRF guard: pull-target hosts resolving to private / loopback /
link-local IPs (including cloud metadata at `169.254.169.254`) are
rejected. Bypass with `AKUA_ALLOW_PRIVATE_HOSTS=1` for local dev.

See [SECURITY.md](../../SECURITY.md) for the threat model and the
complete list of hardening.

## What the SDK doesn't do

- **Helm render** — the embedded template engine (Go→wasip1 via
  wasmtime) is CLI-only. The SDK produces umbrella charts ready for
  any Helm-compatible renderer downstream.
- **Transport error wrapping** — native `fetch` / `DOMException`
  errors pass through unwrapped; catch `AkuaError` for SDK-raised
  faults and handle transport separately.

## Versioning

Tracks `apiVersion: akua.dev/v1alpha1` for manifest shapes. Function
signatures frozen for the `v1alpha1` lifetime.

## License

Apache-2.0
