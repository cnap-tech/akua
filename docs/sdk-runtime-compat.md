# `@akua-dev/sdk` runtime compatibility matrix

> Status as of v0.6.x. Re-verify per release.

The SDK ships as a normal npm package with a per-platform native addon (`@akua-dev/native-{os}-{arch}-{libc}`) loaded via Node-API. The compatibility matrix below captures which runtimes load + drive the addon end-to-end.

## Supported

| Runtime | Version | OCI fetch | Helm engine | Kustomize engine | cosign verify | Notes |
|---|---|---|---|---|---|---|
| **Node.js** | 22.x | ✅ | ✅ | ✅ | ✅ | Primary target. CI sweeps green per push. |
| **Node.js** | 24.x | ✅ | ✅ | ✅ | ✅ | sdk-release.yml + native-release.yml both run on 24 (npm 11+ for OIDC trusted publishing). |
| **Bun** | 1.3+ | ✅ | ✅ | ✅ | ✅ | `task sdk:test` runs entirely under bun. Bun's Node-API impl is compatible. |
| **Deno** | 2.x | ✅ | ✅ | ✅ | ✅ | Loads `@akua-dev/native` via `npm:` specifier. Requires `--allow-read --allow-net --allow-env`. |

## Caveats

- **Bun on `--experimental-strip-types`**: not relevant to the SDK any more — the `dist/` build is plain JS + emitted `.d.ts`. The flag was used for the JSR-era source-only distribution and was dropped from the Taskfile in PR #32.
- **WASI sub-instances**: the SDK doesn't expose a WASI surface itself any more (the napi addon embeds wasmtime inside the host). Runtimes that want sandboxed re-entry (`wasi-host` / browser) are tracked in `docs/spikes/engines-on-wasm32-unknown-unknown.md`.
- **Browser**: no support, by design — Node-API doesn't exist in the browser. The browser path requires the wasm32-unknown-unknown route from the spike, deferred to v0.2.x.

## Smoke verification

`task sdk:test` exercises the full surface against bun:

```sh
task sdk:test    # bun test (covers napi load + render + error routing)
```

For Node + Deno the same test surface is reachable via:

```sh
# Node 22+
cd packages/sdk && node --test src/*.test.ts    # native TS support, no flag needed on 22.6+

# Deno 2+
cd packages/sdk && deno test --allow-all src/*.test.ts
```

These are not part of CI today. When v0.7 lands the CI matrix follow-up, `.github/workflows/ci.yml` will fan out to all three runtimes per push. Tracked at #464.

## What breaks if the matrix shifts

- A runtime that doesn't speak Node-API (Cloudflare Workers, browser, embedded JS) cannot load the addon. The SDK throws on first `loadNapi()` with `@akua-dev/sdk: native addon not loadable`.
- A glibc-only Linux distro (musl-only is unusual; most have both) loads `@akua-dev/native-linux-x64-gnu` automatically via `optionalDependencies` resolution.
- Node 21 and below: not tested. Likely works but unsupported. Use 22+.

## See also

- `crates/akua-napi/` — the native crate.
- `crates/akua-napi/index.js` — the auto-generated platform-loader (what picks the right `@akua-dev/native-*` per host).
- `.github/workflows/native-release.yml` — matrix-build for the 7 per-platform packages.
- `.github/workflows/sdk-release.yml` — SDK publish flow (depends on the native-release matching).
