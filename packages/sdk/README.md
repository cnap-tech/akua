# @akua/sdk

TypeScript SDK for [akua](https://github.com/cnap-tech/akua). Pure-compute verbs run in-process via a bundled WASM module — no `akua` binary required. Network- and OS-dependent verbs shell out to the CLI.

## Install

```sh
bun  add jsr:@akua/sdk
deno add jsr:@akua/sdk
pnpm add jsr:@akua/sdk
npm  install jsr:@akua/sdk
```

Node 20+ / Deno / Bun. Browser in v0.2.0 — the crate + bundler-target build are ready; engine bundling is the gating decision.

## Usage

```ts
import { Akua, AkuaUserError, AkuaRateLimitedError } from '@akua/sdk';

const akua = new Akua();

// WASM, in-process — no binary needed
const yaml = await akua.renderSource('package.k', PACKAGE_K_SOURCE, { replicas: 3 });
const lint = await akua.lint({ package: './package.k' });
const tree = await akua.tree({ workspace: '.' });

// Shell-out — needs `akua` on PATH for verbs that aren't yet WASM
const summary = await akua.render({ package: './package.k', out: './deploy' });
```

Every method returns a typed result validated against a JSON Schema generated from the same Rust `serde` types the CLI emits. Contract drift throws at the parse boundary, not as `undefined.field` later:

```ts
try {
  await akua.render({ package: './package.k', out: './deploy' });
} catch (err) {
  if (err instanceof AkuaRateLimitedError) backoff();
  else if (err instanceof AkuaUserError) console.error(err.structured?.code);
  else throw err;
}
```

## Transport per verb (v0.1.0)

| Verb | Transport | Binary required? |
|---|---|---|
| `renderSource`, `check`, `lint`, `fmt`, `inspect` (package), `tree`, `diff` | WASM | no |
| `render`, `verify`, `version`, `whoami` | shell-out | yes |
| `add`, `publish`, `pull`, `push`, `pack`, `sign`, `lock`, `update`, `cache`, `auth`, `dev`, `repl` | not yet wired (shell-out only via CLI) | yes |
| `inspect` (tarball mode) | deferred to v0.2.0 | — |

Override the binary path when shell-out is needed: `new Akua({ binary: '/path/to/akua' })`.

## Examples

Runnable recipes in [`examples/`](examples/). Run any of them directly — they exercise WASM paths and need no binary:

```sh
bun run packages/sdk/examples/01-render-source.ts
bun run packages/sdk/examples/02-lint-package.ts
bun run packages/sdk/examples/06-diff-renders.ts
```

## Types + schema are derived, not hand-written

- `src/types/*.ts` — per-type TS from `ts-rs` derives on Rust serde types in `akua-core` + `akua-cli`.
- `src/schemas/akua.json` — a single bundled JSON Schema from `schemars`. Polyglot consumers (Python, Go, agents) validate against the same shape.

Drift is guarded by `task sdk:check` — regenerate + `git diff --exit-code`.

## Repo tasks

```sh
task sdk:gen             # regenerate types + schema from Rust
task sdk:check           # regenerate + diff-check (wired into `task ci`)
task sdk:test            # bun test (no binary required for the WASM-backed tests)
task sdk:e2e             # build akua CLI, then run every SDK test with AKUA_E2E=1
task sdk:publish:check   # dry-run `jsr publish`
```

## Release flow

SDK versions float independently of the Rust crate version — a wrapper-layer fix ships without rebuilding the binary.

1. Land changes on `main`; `task ci` must be green (includes `sdk:check` drift guard).
2. `task sdk:publish:check` locally to confirm the JSR manifest + slow-type check pass.
3. Push tag `sdk-v<semver>`:

   ```sh
   git tag sdk-v0.1.0
   git push origin sdk-v0.1.0
   ```

4. `.github/workflows/sdk-release.yml` regenerates types + schema on the runner (so a stale local checkout can't leak), runs the drift guard, overwrites `jsr.json.version` from the tag suffix, and publishes via JSR's OIDC auth (no PAT).

### Publish guards

Three layers, stackable:

- **JSR auth**: `bunx jsr publish` without `--dry-run` requires either an OIDC-signed CI run or an interactive JSR auth token. No one can publish by typo-ing a command in a shell without authenticating.
- **Tag gate**: the workflow only runs on `sdk-v*` tags (`workflow_dispatch` supports dry-run only by default).
- **Dirty-check**: `jsr publish` refuses to publish without `--allow-dirty` when the working tree has uncommitted changes.

`package.json` has `"private": true`, but note that JSR reads `jsr.json` preferentially — that flag is only effective against `npm publish`, not `jsr publish`. The three guards above are what actually prevent accidents.

## Still coming

- Browser target — bundler-build is staged; blocked on helm/kustomize on `wasm32-unknown-unknown` (see [docs/spikes/engines-on-wasm32-unknown-unknown.md](../../docs/spikes/engines-on-wasm32-unknown-unknown.md)).
- Remaining network / OS-dependent verbs (`add`, `publish`, `pull`, `push`, `lock`, `update`, `cache`, `auth`, `dev`). Each will ship as a thin shell-out wrapper; the WASM bundle stays focused on pure-compute verbs.
- Signed OCI publish of `src/schemas/akua.json` to `ghcr.io/cnap-tech/akua/schemas:v1` for polyglot consumers.
