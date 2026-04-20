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

  // Authoring + build
  init(opts: InitOptions): Promise<InitResult>;
  add(opts: AddOptions): Promise<AddResult>;
  render(opts: RenderOptions): Promise<RenderResult>;
  diff(a: string, b: string, opts?: DiffOptions): Promise<DiffResult>;
  attest(opts: AttestOptions): Promise<AttestResult>;
  publish(opts: PublishOptions): Promise<PublishResult>;
  pull(ref: string, opts?: PullOptions): Promise<PullResult>;
  inspect(ref: string, opts?: InspectOptions): Promise<InspectResult>;
  export(opts: ExportOptions): Promise<ExportResult>;

  // Develop (matches CLI verbs: test, fmt, check, lint, bench, cov, eval, repl)
  test(opts?: TestOptions): Promise<TestResult>;
  fmt(opts?: FmtOptions): Promise<FmtResult>;
  check(opts?: CheckOptions): Promise<CheckResult>;
  lint(opts?: LintOptions): Promise<LintResult>;
  bench(opts?: BenchOptions): Promise<BenchResult>;
  cov(opts?: CovOptions): Promise<CovResult>;
  eval(query: string, opts?: EvalOptions): Promise<EvalResult>;

  // Deploy loop
  dev(opts?: DevOptions): Promise<DevSession>;
  verify(opts?: VerifyOptions): Promise<VerifyResult>;  // checks akua.toml ↔ akua.lock

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
  engine?: 'auto' | 'embedded' | 'shell';  // engine selection default
  signal?: AbortSignal;        // cancel long-running ops
}
```

### Universal option — `engine`

Methods that invoke an engine (render, test, lint, bench, policy.check, etc.) accept an `engine?: 'auto' | 'embedded' | 'shell'` override. Default behavior: embedded engine bundled with akua. Shell-out is an escape hatch when a specific engine version on `$PATH` is required. See [embedded-engines.md](embedded-engines.md).

### Universal option — `idempotencyKey`

Write methods (`publish`, `deploy.apply`, `secret.rotate`, etc.) accept `idempotencyKey?: string`. If the same key is presented twice, the second call is a no-op that returns the original result. See [cli-contract.md §3](cli-contract.md#3-writes-are-idempotent).

---

## Package API

```ts
interface PackageAPI {
  // Authoring + build
  init(opts: InitOptions): Promise<InitResult>;
  inspect(ref: string, opts?: InspectOptions): Promise<PackageInfo>;
  diff(a: string, b: string, opts?: DiffOptions): Promise<DiffResult>;
  render(opts: RenderOptions): Promise<RenderResult>;
  attest(opts: AttestOptions): Promise<AttestResult>;
  publish(opts: PublishOptions): Promise<PublishResult>;
  pull(ref: string, opts?: PullOptions): Promise<PullResult>;
  export(opts: PackageExportOptions): Promise<PackageExportResult>;

  // Develop
  test(opts?: PackageTestOptions): Promise<TestResult>;
  check(opts?: CheckOptions): Promise<CheckResult>;       // syntax + type pass only
  lint(opts?: LintOptions): Promise<LintResult>;          // kcl lint + cross-engine
  fmt(opts?: FmtOptions): Promise<FmtResult>;
}

interface PackageExportOptions {
  path?: string;
  format: 'json-schema' | 'openapi' | 'yaml' | 'oci-bundle';
  outFile?: string;
  pretty?: boolean;
  engine?: 'auto' | 'embedded' | 'shell';
}

interface PackageExportResult {
  format: string;
  bytes: number;
  path?: string;
  content?: string;   // when outFile is omitted
}

interface PackageTestOptions {
  path?: string;
  filter?: RegExp | string;
  coverage?: boolean;
  golden?: 'verify' | 'regenerate';
  watch?: boolean;
  engine?: 'auto' | 'embedded' | 'shell';
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

## Document API

akua does not specify an App / Environment / Cluster / Secret vocabulary. The SDK therefore exposes **generic document operations** that work against whatever KCL schemas the workspace declares. If your workspace authors an `App` schema, `akua.doc.list({ kind: 'App' })` finds them; the SDK doesn't know what fields are inside.

```ts
interface DocumentAPI {
  // Discover user-authored KCL documents in the current workspace
  list(opts?: ListOptions): Promise<DocumentRef[]>;

  // Read a specific document by path, producing the typed KCL value
  // the workspace's schema declares
  get<T = unknown>(path: string): Promise<T>;

  // Apply a document (kick off render + deploy according to its kind's
  // handler, which the workspace configures)
  apply(path: string, opts?: ApplyOptions): Promise<DeployHandle>;

  // Export a KCL document as YAML/JSON (derived view)
  export(path: string, opts: { format: 'yaml' | 'json' }): Promise<string>;
}

interface ListOptions {
  kind?: string;                              // filter by declared KCL schema name
  filter?: Record<string, unknown>;           // field predicates (e.g. { 'spec.env': 'production' })
  under?: string;                             // directory scope
}

interface DocumentRef {
  path: string;                               // workspace-relative path to the .k file
  kind: string;                               // schema name declared in the KCL program
  name: string;                               // the document's top-level name field (convention)
}
```

Typed access requires the workspace to generate TypeScript types from its own schemas:

```ts
// Generated from the workspace's own schemas/app.k
import type { App } from './generated/schemas';

const apps = await akua.doc.list({ kind: 'App', filter: { 'spec.env': 'production' } });
for (const ref of apps) {
  const app = await akua.doc.get<App>(ref.path);
  // app is fully typed against the workspace's App schema
}
```

Type generation is a workspace-local concern: `akua export schemas/app.k --format=typescript > generated/app.ts`. akua does not ship an `App` TypeScript type because it does not specify an `App` schema.

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
  to: 'argo' | 'flux' | 'kro' | 'helm' | 'kubectl' | string;
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
  // Evaluation
  check(opts: PolicyCheckOptions): Promise<PolicyVerdict>;

  // Authoring
  tiers(): Promise<PolicyTierInfo[]>;
  show(tier: string): Promise<PolicyDefinition>;
  diff(a: string, b: string): Promise<PolicyDiff>;
  install(tier: string, opts?: { from?: string }): Promise<void>;
  fork(tier: string, opts: { as: string }): Promise<PolicyDefinition>;
  publish(tier: string, opts?: { idempotencyKey?: string }): Promise<PublishResult>;
  export(tier: string, opts: { format: 'rego-bundle' | 'yaml' }): Promise<string>;

  // Develop
  test(opts?: PolicyTestOptions): Promise<TestResult>;
  explain(query: string, opts?: PolicyExplainOptions): Promise<PolicyTrace>;
  bench(opts?: PolicyBenchOptions): Promise<BenchResult>;
  coverage(opts?: { min?: number; format?: 'json' | 'lcov' }): Promise<CovResult>;
  fmt(opts?: FmtOptions): Promise<FmtResult>;
  lint(opts?: LintOptions): Promise<LintResult>;  // Regal + cross-engine
  eval(query: string, opts?: { input?: unknown }): Promise<EvalResult>;
}

interface PolicyTestOptions {
  path?: string;
  filter?: RegExp | string;
  coverage?: boolean;
  watch?: boolean;
  engine?: 'auto' | 'embedded' | 'shell';
}

interface PolicyExplainOptions {
  input?: unknown;            // the document to evaluate against
  depth?: 'notes' | 'fails' | 'full' | 'debug';   // OPA --explain mode
  data?: string;              // policy bundle directory
}

interface PolicyTrace {
  query: string;
  verdict: 'allow' | 'deny' | 'needs-approval';
  steps: Array<{
    rule: string;
    evaluated: boolean;
    result?: unknown;
    location?: { file: string; line: number };
  }>;
}

interface PolicyBenchOptions {
  tier?: string;
  input?: unknown;
  iterations?: number;
  p99MaxMs?: number;          // fail if p99 exceeds (CI gate)
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

## Shared result types

Types used by the develop-verb methods (`test`, `fmt`, `lint`, `check`, `bench`, `cov`, `eval`, `export`) across Package and Policy APIs.

```ts
interface TestResult {
  summary: { passed: number; failed: number; skipped: number; durationMs: number };
  results: Array<{
    file: string;
    test: string;
    status: 'pass' | 'fail' | 'skip';
    durationMs: number;
    message?: string;
  }>;
  coverage?: { overall: number; byRule: Record<string, number> };
}

interface FmtResult {
  formatted: string[];        // files modified
  unchanged: string[];        // files already correct
}

interface LintResult {
  issues: Array<{
    file: string;
    line: number;
    col?: number;
    rule: string;
    severity: 'warn' | 'error';
    message: string;
    fix?: string;
  }>;
  summary: { warn: number; error: number };
}

interface CheckResult {
  valid: boolean;
  summary: { files: number; errors: number; warnings: number; durationMs: number };
  errors?: Array<{ file: string; line: number; code: string; message: string; suggestion?: string }>;
}

interface BenchResult {
  benchmarks: Array<{
    name: string;
    iterations: number;
    totalMs: number;
    meanUs: number;
    p99Us: number;
    rulesEvaluated?: number;
  }>;
}

interface CovResult {
  overall: number;
  byRule: Record<string, number>;
  bySchema?: Record<string, number>;
  uncovered: string[];
}

interface EvalResult {
  lang: 'rego' | 'kcl';
  query: string;
  result: unknown;
  durationMs: number;
}

interface ExportOptions {
  // Top-level export — dispatches to PackageAPI.export / PolicyAPI.export /
  // AppAPI.export / etc. based on the target.
  target?: string;            // "app:checkout", "policy:tier/production", "package:."
  format: 'json-schema' | 'openapi' | 'yaml' | 'json' | 'rego-bundle' | 'oci-bundle';
  outFile?: string;
  pretty?: boolean;
}

interface ExportResult {
  format: string;
  bytes: number;
  path?: string;
  content?: string;
}

interface VerifyResult {
  valid: boolean;                    // akua.toml ↔ akua.lock consistency
  issues: Array<{
    dep: string;
    issue: 'digest-mismatch' | 'signature-invalid' | 'unsigned' | 'missing';
    details: string;
  }>;
}
```

## Identity — agent context

`whoami()` returns the current identity including any detected agent context:

```ts
interface Identity {
  identity: string;
  registries: Array<{ url: string; user: string; expiresAt?: string }>;
  scopes: string[];
  agentContext?: {
    detected: boolean;
    agent?: string;             // 'claude-code' | 'cursor' | 'codex' | 'gemini-cli' | ...
    sourceEnv?: string;         // the env var that triggered detection
  };
}
```

When an agent runs `akua whoami --json`, the `agentContext` field is populated from the env-var-based auto-detection (see [cli-contract.md §1.5](cli-contract.md#15-agent-context-auto-detection)). Useful for agents verifying they're operating inside the expected runtime.

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

## Spec cross-references

The SDK's types mirror the underlying format specs. For the authoritative data shapes:

- **Package** (KCL program, the one akua-specified shape) — [package-format.md](package-format.md)
- **Policy** (Rego host + pluggable engines) — [policy-format.md](policy-format.md)
- **Lockfile** (`akua.toml` + `akua.lock`) — [lockfile-format.md](lockfile-format.md)
- **CLI contract** (invariants every method honors) — [cli-contract.md](cli-contract.md)
- **CLI reference** (the verbs the SDK methods mirror) — [cli.md](cli.md)
- **Embedded engines** (`engine?: 'auto' | 'embedded' | 'shell'`) — [embedded-engines.md](embedded-engines.md)
- **Agent usage + auto-detection** — [agent-usage.md](agent-usage.md)

TypeScript types in `@akua/sdk` are generated from the same akua-specified schemas the CLI consumes (Package, Policy, akua.toml — the shapes akua owns), so a field shape in the SDK always matches the file-format spec. When the spec evolves, the generated types follow on the next `@akua/sdk` release.
