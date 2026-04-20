# @akua/sdk — TypeScript SDK

Programmatic access to akua capabilities for Node.js and the browser. Mirrors the CLI surface where appropriate; differs where the context demands it.

---

## Install

```sh
bun add @akua/sdk
# or
npm install @akua/sdk
```

Published to [JSR](https://jsr.io/@akua/sdk). ESM-only. Node 20+ or any modern browser.

---

## Two entry points

```ts
import { Akua } from '@akua/sdk';           // Node — full surface
import { Akua } from '@akua/sdk/browser';   // browser — read-only subset
```

The Node SDK wraps the full CLI contract programmatically. The browser SDK wraps the subset that's safe to run client-side: inspection, rendering, diffing, verification. No writes, no secrets, no deploy.

---

## Design principles

1. **One class per primitive.** `Akua` is the root; everything hangs off it.
2. **Async everywhere.** Every operation returns a Promise. No sync I/O.
3. **Typed end-to-end.** Full TypeScript types for every input and output.
4. **Thin wrapper, not a framework.** The SDK shells out to the `akua` binary or uses the same Rust core via WASM; it doesn't reimplement.
5. **Results mirror `--json` output.** If you know the CLI, you know the SDK's return shapes.
6. **Idempotent writes.** Every write method accepts `idempotencyKey`; if omitted, one is generated.
7. **Streaming where the CLI streams.** `dev()` and long-running operations return AsyncIterables.

---

## Quickstart (Node)

```ts
import { Akua } from '@akua/sdk';

const akua = new Akua({
  binary: 'akua',               // path; default: 'akua' (PATH)
  registry: 'oci://ghcr.io/me', // default registry
  token: process.env.AKUA_TOKEN // optional; otherwise uses system credential store
});

// render a package
const result = await akua.render({
  path: './my-pkg',
  inputs: { appName: 'checkout', hostname: 'checkout.example.com' }
});
console.log(result.outputs);  // [{ name, format, hash, manifests }]

// publish
const pub = await akua.publish({
  path: './my-pkg',
  to: 'oci://pkg.akua.dev/checkout',
  tag: '1.0.0'
});
console.log(pub.digest);

// deploy + wait
const handle = await akua.deploy({ app: 'checkout', to: 'argo' });
const status = await handle.waitReady({ timeout: '5m' });
```

---

## Root — `Akua`

```ts
class Akua {
  constructor(opts?: AkuaOptions);

  readonly package: PackageAPI;
  readonly app: AppAPI;
  readonly deploy: DeployAPI;
  readonly secret: SecretAPI;
  readonly policy: PolicyAPI;
  readonly audit: AuditAPI;
  readonly query: QueryAPI;
  readonly infra: InfraAPI;
  readonly rollout: RolloutAPI;
  readonly registry: RegistryAPI;

  // Direct verb equivalents (convenience)
  init(opts: InitOptions): Promise<InitResult>;
  add(opts: AddOptions): Promise<AddResult>;
  lint(opts: LintOptions): Promise<LintResult>;
  render(opts: RenderOptions): Promise<RenderResult>;
  diff(a: string, b: string, opts?: DiffOptions): Promise<DiffResult>;
  attest(opts: AttestOptions): Promise<AttestResult>;
  publish(opts: PublishOptions): Promise<PublishResult>;
  pull(ref: string, opts?: PullOptions): Promise<PullResult>;
  inspect(ref: string, opts?: InspectOptions): Promise<InspectResult>;
  dev(opts?: DevOptions): Promise<DevSession>;

  // Session
  login(opts: LoginOptions): Promise<void>;
  logout(registry?: string): Promise<void>;
  whoami(): Promise<Identity>;
  version(): Promise<VersionInfo>;

  // Discovery
  help(verb?: string): Promise<CommandTree>;
}
```

```ts
interface AkuaOptions {
  binary?: string;             // path to akua binary (default: 'akua')
  registry?: string;           // default OCI registry
  token?: string;              // API token; if omitted, uses credential store
  cacheDir?: string;           // override cache location
  timeout?: string;            // default timeout for operations (e.g. '5m')
  logLevel?: 'debug'|'info'|'warn'|'error';
  signal?: AbortSignal;        // cancel long-running ops
}
```

---

## Package API

```ts
interface PackageAPI {
  init(opts: InitOptions): Promise<InitResult>;
  inspect(ref: string, opts?: InspectOptions): Promise<PackageInfo>;
  diff(a: string, b: string, opts?: DiffOptions): Promise<DiffResult>;
  render(opts: RenderOptions): Promise<RenderResult>;
  attest(opts: AttestOptions): Promise<AttestResult>;
  publish(opts: PublishOptions): Promise<PublishResult>;
  pull(ref: string, opts?: PullOptions): Promise<PullResult>;
  lint(opts: LintOptions): Promise<LintResult>;
}

interface InitOptions {
  name?: string;
  template?: 'app' | 'app-with-db' | 'umbrella' | 'platform-std' | 'empty';
  targetDir?: string;
  noGit?: boolean;
}

interface InitResult {
  name: string;
  path: string;
  template: string;
  files: string[];
}

interface RenderOptions {
  path?: string;               // package directory (default: cwd)
  inputs?: Record<string, unknown>;
  inputsFile?: string;
  output?: string;             // named output; default: all
  outDir?: string;
  dryRun?: boolean;
  format?: 'raw' | 'helm' | 'rgd' | 'xr' | 'oci';
}

interface RenderResult {
  outputs: RenderedOutput[];
  policy: PolicyVerdict;
  attestationPath?: string;
}

interface RenderedOutput {
  name: string;
  format: string;
  target: string;
  manifestCount: number;
  hash: string;                // sha256
  manifestPaths: string[];
}

interface PublishOptions {
  path?: string;
  to?: string;                 // OCI ref; default from package metadata
  tag?: string;
  sign?: boolean;              // default: true
  attest?: boolean;            // default: true
  public?: boolean;
  idempotencyKey?: string;
}

interface PublishResult {
  package: string;
  version: string;
  digest: string;
  signed: boolean;
  attestationDigest?: string;
  sizeBytes: number;
  uploadDurationMs: number;
}

interface DiffResult {
  schema: {
    added: string[];
    removed: string[];
    typeChanged: Array<{ path: string; from: string; to: string }>;
    defaultChanged: Array<{ path: string; from: unknown; to: unknown }>;
  };
  sources: {
    added: SourceRef[];
    removed: SourceRef[];
    versionChanged: Array<{ name: string; from: string; to: string }>;
  };
  manifests: { added: number; removed: number; modified: number };
  policyCompat: 'allow' | 'deny' | 'needs-approval';
}
```

---

## App API

```ts
interface AppAPI {
  list(opts?: { env?: string; namespace?: string }): Promise<App[]>;
  get(name: string): Promise<App>;
  apply(app: AppSpec, opts?: ApplyOptions): Promise<DeployHandle>;
  delete(name: string, opts?: { force?: boolean }): Promise<void>;
}

interface AppSpec {
  name: string;
  namespace?: string;
  labels?: Record<string, string>;
  spec: {
    package: string;           // OCI ref
    inputs: Record<string, unknown>;
    policy?: string;
    env?: string;
  };
}

interface App extends AppSpec {
  status: {
    phase: 'pending' | 'reconciling' | 'healthy' | 'degraded' | 'failed';
    lastDeploy?: { changeId: string; at: string };
    ready: number;
    total: number;
  };
}
```

---

## Deploy API

```ts
interface DeployAPI {
  apply(opts: DeployOptions): Promise<DeployHandle>;
  status(handle: string): Promise<DeployStatus>;
  history(opts?: { service?: string; last?: number }): Promise<DeployRecord[]>;
  rollback(changeId: string, opts?: { dryRun?: boolean }): Promise<DeployHandle>;
  cancel(handle: string): Promise<void>;
}

interface DeployOptions {
  app?: string;
  path?: string;
  to: 'argo' | 'flux' | 'kro' | 'helm' | 'kubectl' | 'fly' | 'cf-workers' | 'akua' | string;
  inputs?: Record<string, unknown>;
  idempotencyKey?: string;
}

interface DeployHandle {
  id: string;
  target: string;
  status: () => Promise<DeployStatus>;
  wait(opts?: { timeout?: string }): Promise<DeployStatus>;
  waitReady(opts?: { timeout?: string }): Promise<DeployStatus>;
  cancel(): Promise<void>;
  stream(): AsyncIterable<DeployEvent>;
}

interface DeployStatus {
  handle: string;
  phase: 'pending' | 'applying' | 'reconciling' | 'healthy' | 'degraded' | 'failed';
  health: 'healthy' | 'degraded' | 'unknown';
  ready: number;
  total: number;
  startedAt: string;
  lastEvent: string;
  prUrl?: string;
}
```

---

## Secret API

```ts
interface SecretAPI {
  list(opts?: { store?: string }): Promise<SecretSummary[]>;
  get(name: string): Promise<SecretRef>;               // returns ref, never raw value
  add(opts: SecretAddOptions): Promise<SecretRef>;
  rotate(name: string, opts?: { idempotencyKey?: string }): Promise<SecretRef>;
  grant(name: string, opts: GrantOptions): Promise<void>;
  revoke(name: string, opts: { from: string }): Promise<void>;
  trace(name: string): Promise<SecretTrace>;
  delete(name: string): Promise<void>;                  // needs-approval in most tiers
}

interface SecretRef {
  name: string;
  store: string;
  ref: string;                  // e.g. "vault://secrets/x/api-key"
  rotation?: { policy: string; lastRotated: string; nextDue: string };
}

interface SecretTrace extends SecretRef {
  grants: Array<{ service: string; scope: 'read' | 'write'; grantedAt: string }>;
  lastAccess?: string;
}
```

---

## Policy API

```ts
interface PolicyAPI {
  check(opts: PolicyCheckOptions): Promise<PolicyVerdict>;
  tiers(): Promise<PolicyTierInfo[]>;
  show(tier: string): Promise<PolicyDefinition>;
  diff(a: string, b: string): Promise<PolicyDiff>;
  install(tier: string, opts?: { from?: string }): Promise<void>;
  fork(tier: string, opts: { as: string }): Promise<PolicyDefinition>;
  publish(tier: string): Promise<PublishResult>;
}

interface PolicyCheckOptions {
  tier?: string;
  targetPath?: string;           // directory of rendered manifests
  manifests?: unknown[];         // or pass them inline
}

interface PolicyVerdict {
  tier: string;
  verdict: 'allow' | 'deny' | 'needs-approval';
  checks: Record<string, 'pass' | 'warn' | 'fail'>;
  failing: Array<{
    rule: string;
    resource: string;
    reason: string;
    suggestedFix?: string;
  }>;
  approvers?: string[];
}
```

---

## Audit API

```ts
interface AuditAPI {
  explain(id: string): Promise<AuditExplanation>;
  trace(opts: TraceOptions): Promise<AuditEvent[]>;
  search(opts: SearchOptions): Promise<AuditEvent[]>;
  export(opts: ExportOptions): Promise<ReadableStream<Uint8Array>>;
  who(resource: string): Promise<PermissionList>;
}

interface AuditExplanation {
  incidentId: string;
  trigger: { type: string; service?: string; at: string };
  rootCause: {
    changeId: string;
    actor: string;
    reason: string;
    committedAt: string;
  };
  resolution?: {
    action: 'rollback' | 'forward-fix' | 'accept';
    changeId?: string;
    actor?: string;
    completedAt?: string;
  };
  durationMinutes: number;
  learned?: string;
}
```

---

## Query API

```ts
interface QueryAPI {
  run(expr: string, opts?: QueryOptions): Promise<QueryResult>;
  stream(expr: string, opts?: QueryOptions): AsyncIterable<QueryResult>;
}

interface QueryOptions {
  backend?: 'prometheus' | 'loki' | 'tempo' | 'auto';
  since?: string;                // duration, e.g. '1h'
  until?: string;
  step?: string;                 // for stream
}

interface QueryResult {
  query: string;
  backend: string;
  result: {
    value?: number;
    values?: Array<[timestamp: number, value: number]>;
    baseline?: number;
    changePct?: number;
    samples?: number;
  };
}
```

---

## Rollout API

```ts
interface RolloutAPI {
  plan(spec: RolloutSpec): Promise<RolloutPlan>;
  apply(spec: RolloutSpec, opts?: ApplyOptions): Promise<RolloutHandle>;
  status(handle: string): Promise<RolloutStatus>;
  pause(handle: string): Promise<void>;
  resume(handle: string): Promise<void>;
  abort(handle: string): Promise<void>;     // triggers rollback
}

interface RolloutSpec {
  name: string;
  changes: Array<{ repo: string; path: string; patch: Record<string, unknown> }>;
  strategy?: 'parallel' | 'staged' | 'canary';
  batchSize?: number;
  soak?: string;
  policyTier?: string;
}

interface RolloutHandle {
  id: string;
  status(): Promise<RolloutStatus>;
  wait(opts?: { timeout?: string }): Promise<RolloutStatus>;
  stream(): AsyncIterable<RolloutEvent>;
}

interface RolloutStatus {
  handle: string;
  phase: 'planning' | 'running' | 'paused' | 'complete' | 'aborted' | 'failed';
  stages: Array<{
    name: string;
    status: 'pending' | 'running' | 'soak' | 'complete' | 'failed';
    completedAt?: string;
  }>;
  currentStage?: string;
  progress: { done: number; total: number };
}
```

---

## Dev session (streaming)

```ts
interface DevOptions {
  path?: string;
  target?: 'local' | 'dry-run' | `cluster:${string}`;
  port?: number;
  policy?: string;
  fresh?: boolean;
  inputs?: Record<string, unknown>;
}

interface DevSession {
  url: string;                          // browser UI URL
  target: string;
  stop(): Promise<void>;
  events(): AsyncIterable<DevEvent>;
  on(event: 'render', handler: (e: RenderEvent) => void): () => void;
  on(event: 'apply', handler: (e: ApplyEvent) => void): () => void;
  on(event: 'policy', handler: (e: PolicyEvent) => void): () => void;
  on(event: 'error', handler: (e: DevErrorEvent) => void): () => void;
}

interface DevEvent {
  t: number;                            // unix ms
  stage: 'parse' | 'validate' | 'render' | 'policy' | 'diff' | 'apply' | 'reconcile';
  app?: string;
  resource?: string;
  durationMs?: number;
  status: 'ok' | 'warn' | 'error';
  outputHash?: string;
  details?: Record<string, unknown>;
}
```

### Usage example

```ts
const session = await akua.dev({ path: './my-workspace' });
console.log('browser UI:', session.url);

// Events stream
for await (const event of session.events()) {
  if (event.stage === 'reconcile' && event.status === 'ok') {
    console.log(`${event.resource} reconciled in ${event.durationMs}ms`);
  }
  if (event.status === 'error') {
    break;
  }
}

await session.stop();
```

---

## Registry API

```ts
interface RegistryAPI {
  login(registry: string, opts: LoginOptions): Promise<void>;
  logout(registry?: string): Promise<void>;
  list(): Promise<Array<{ url: string; user: string; expiresAt?: string }>>;
  verify(ref: string): Promise<VerificationResult>;
}

interface VerificationResult {
  ref: string;
  digest: string;
  signed: boolean;
  signer?: string;
  signatureValid: boolean;
  attestations: Array<{
    predicateType: string;
    subject: { digest: string };
    issuer?: string;
  }>;
}
```

---

## Error handling

Every SDK method throws a typed `AkuaError` on failure:

```ts
class AkuaError extends Error {
  code: string;                         // e.g. 'E_SCHEMA_INVALID'
  exitCode: number;                     // CLI exit code (1-6)
  path?: string;
  field?: string;
  suggestion?: string;
  docsUrl?: string;
  cause?: unknown;
}
```

Agents branch on `code` or `exitCode`:

```ts
try {
  await akua.deploy({ to: 'argo', app: 'checkout' });
} catch (err) {
  if (err instanceof AkuaError) {
    if (err.exitCode === 3) {
      // policy denied; check err.field for which rule
    } else if (err.exitCode === 5) {
      // needs approval; wait for human
    } else {
      throw err;
    }
  } else {
    throw err;
  }
}
```

---

## Browser SDK — `@akua/sdk/browser`

Same class, subset of methods. Designed for the playground at `akua.dev` and for embedded audit UIs.

```ts
import { Akua } from '@akua/sdk/browser';

const akua = new Akua();

// Read-only operations work
const pkg = await akua.inspect('oci://pkg.akua.dev/payments-api:3.2');
const diff = await akua.diff('v1', 'v2');
const rendered = await akua.render({
  ref: 'oci://pkg.akua.dev/payments-api:3.2',
  inputs: { hostname: 'demo.example.com' }
});

// Write operations throw
try {
  await akua.publish({...});
} catch (err) {
  // err.code === 'E_WRITE_UNSUPPORTED_IN_BROWSER'
}
```

The browser SDK loads the Rust core as a WASM module (once, lazily) and uses it to render, diff, and verify. No CLI shell-out. No backend calls except to fetch OCI artifacts from public registries (CORS-permitting).

### Available in browser

- `akua.inspect`
- `akua.diff`
- `akua.render` (with some restrictions on sources — public OCI/HTTP only)
- `akua.verify`
- `akua.help`

### Not available in browser

- `akua.publish`, `akua.attest`
- `akua.deploy`, `akua.rollout`
- `akua.secret.*`, `akua.audit.*`, `akua.query.*`
- `akua.login` (but OAuth via popup works if the host site sets it up)

---

## Server-side contexts (CI, webhook handlers)

For CI jobs or webhook handlers that need a subset of the SDK without spawning subprocesses, future versions will ship `@akua/sdk/lib` — a pure-WASM library variant with the same API shape but no CLI dependency. Tracked for v0.3.

---

## Stability contract

- Public types (`AkuaOptions`, all API interfaces, error shapes) are stable from v1.0.
- Private types (prefixed with `_`) can change between minors.
- New methods can be added without bumping major.
- Method removal requires deprecation cycle.

---

## Relationship to the CLI

The SDK is a thin wrapper. Internally it either:
- Spawns the `akua` binary with `--json` and parses the result, or
- (v0.3+) calls the Rust core via WASM directly in-process.

This means:
- Every SDK feature has a CLI equivalent.
- CLI behavior and SDK behavior are identical for the same input.
- Contract changes to the CLI are reflected one-for-one in the SDK.

You can always reach for the CLI if the SDK is missing something. You can always reach for the SDK if scripting from Node is nicer than shell.

---

## What's not in the SDK

- UI components. The browser playground at `akua.dev` has its own UI layer; the SDK doesn't ship React components.
- Reconciler-specific libraries. The SDK invokes `akua deploy`; it doesn't re-implement Argo's client or Flux's client.
- Custom target drivers. Adding a new `--to=<driver>` target is a CLI plugin, not an SDK extension.
