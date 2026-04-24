# @akua/sdk

TypeScript SDK for the [akua](https://github.com/cnap-tech/akua) CLI.
Shells out to `akua <verb> --json`, parses stdout, returns values
typed by TypeScript types auto-generated from the same Rust serde
types the CLI uses.

## Usage

```ts
import { Akua, AkuaUserError, AkuaRateLimitedError } from '@akua/sdk';

const akua = new Akua();
const v = await akua.version();
console.log(v.version);  // VersionOutput.version: string

try {
  await akua.whoami();
} catch (err) {
  if (err instanceof AkuaRateLimitedError) backoff();
  else if (err instanceof AkuaUserError) console.error(err.structured?.code);
  else throw err;
}
```

The SDK needs an `akua` binary on `$PATH` (override via `new Akua({ binary: '...' })`).

## Types + schema are derived, not hand-written

- `src/types/*.ts` — per-type TS from `ts-rs` derives on Rust serde
  types in `akua-core` + `akua-cli`.
- `src/schemas/akua.json` — single bundled JSON Schema doc from
  `schemars` derives. Polyglot consumers (Python, Go, another Rust,
  agents) can validate against this directly — it's the same shape,
  different format.

Every SDK method validates stdout against the bundle before returning,
so contract drift between the Rust source and the SDK's compile-time
types throws a typed `AkuaContractError` at the parse boundary rather
than surfacing later as `undefined.field`.

## Repo tasks

```sh
task sdk:gen             # regenerate src/types + src/schemas from Rust
task sdk:check           # regenerate then fail on diff (drift guard — wired into `task ci`)
task sdk:test            # bun test packages/sdk
task sdk:publish:check   # dry-run `jsr publish` — catches manifest / slow-type issues
```

## Release flow

SDK versions float independently of the Rust crate version — a wrapper-
layer fix ships without rebuilding the binary.

1. Land changes on `main`; `task ci` must be green (includes
   `sdk:check` drift guard).
2. `task sdk:publish:check` locally to confirm the JSR manifest +
   slow-type check pass.
3. Push tag `sdk-v<semver>`:

   ```sh
   git tag sdk-v0.1.0
   git push origin sdk-v0.1.0
   ```

4. `.github/workflows/sdk-release.yml` regenerates types + schema on
   the runner (so a stale local checkout can't leak), runs the drift
   guard, overwrites `jsr.json.version` from the tag suffix, and
   publishes via JSR's OIDC auth (no PAT).

### Publish guards

Three layers, stackable:

- **JSR auth**: `bunx jsr publish` without `--dry-run` requires either
  an OIDC-signed CI run or an interactive JSR auth token. No one can
  publish by typo-ing a command in a shell without authenticating.
- **Tag gate**: the workflow only runs on `sdk-v*` tags
  (`workflow_dispatch` supports dry-run only by default).
- **Dirty-check**: `jsr publish` refuses to publish without
  `--allow-dirty` when the working tree has uncommitted changes.

`package.json` has `"private": true`, but note that JSR reads
`jsr.json` preferentially — that flag is only effective against
`npm publish`, not `jsr publish`. The three guards above are what
actually prevent accidents.

## Not yet shipped

- `@akua/sdk/browser` — read-only subset (inspect / render / diff /
  verify) that runs via WASM instead of shell-out. Blocked on
  reintroducing the `akua-wasm` crate + WASM-safe engines.
- Remaining ~29 verbs from [`docs/sdk.md`](../../docs/sdk.md).
  Each one is a few lines — the pattern is proven by `version` + `whoami`.
- Signed OCI publish of `src/schemas/akua.json` to
  `ghcr.io/cnap-tech/akua/schemas:v1` for polyglot consumers.
