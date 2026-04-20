# Implementation Plan

> **Purpose.** Concrete, agent-executable plan for pivoting the akua codebase from v0.3 (`package.yaml` / JSON-Schema era) to the interface-spec target (KCL-authored Packages, Rego-authored Policies, 30 verbs, `@akua/sdk` parity, embedded multi-engine pipeline). Phasing lives in [`roadmap.md`](./roadmap.md); this doc is the "how."

akua is a pre-alpha project; the pivot is a **surgical rewrite, not a greenfield one.** Most of v0.3 (OCI client, fetch, security guards, Helm engine embedding, attestation) is carry-forward. The authoring vocabulary and verb surface are what change.

---

## 1. Carry-forward vs rewrite

### Carry forward (minimal change)

| Component | Why |
|---|---|
| `crates/akua-core/src/fetch/` | OCI + HTTP chart-dep fetcher, SSRF guard, tar-bomb rejection — all still correct |
| `crates/akua-core/src/publish.rs` | Pure-Rust OCI push with Helm-compat media types — reusable |
| `crates/akua-core/src/attest.rs` | SLSA v1 predicate emission — reusable |
| `crates/akua-core/src/metadata.rs` | `.akua/metadata.yaml` provenance — still useful |
| `crates/helm-engine-wasm/` | Embedded Helm v4 via wasip1 — the `helm.template()` callable target |
| `crates/akua-core/src/tar_security.rs` (implicit) | Tar extraction guards — P0 security fixes stay |
| `packages/sdk/src/oci.ts` | `@akua/sdk` OCI pull primitives — reusable |

### Rewrite (shape changes)

| Component | What changes |
|---|---|
| `crates/akua-cli/src/main.rs` | 30-verb surface replacing current ~10 verbs; honors [`cli-contract.md`](./cli-contract.md) universally |
| `crates/akua-core/src/schema.rs` | `x-user-input` / `x-install` vocabulary → `@ui` decorators on KCL schemas (see [`package-format.md`](./package-format.md)) |
| `crates/akua-core/src/source.rs` | `package.yaml` + `engine:` field loader → `Package.k` KCL loader |
| `crates/akua-core/src/engine/` | Engine trait kept; impls become **callables from KCL** (`helm.template()`, `kustomize.build()`, `rgd.instantiate()`) instead of `Engine::prepare()` invoked by umbrella assembler |
| `crates/akua-core/src/render.rs` | Umbrella assembly → KCL program execution; `resources = [...]` + `outputs = [...]` shape per [`package-format.md`](./package-format.md) |
| `crates/akua-core/src/values.rs` | CEL transforms → KCL-native composition (schemas + `check:` blocks replace runtime CEL in most cases) |
| `packages/sdk/src/*.ts` | JSR publish kept; API surface expanded for parity with 30-verb CLI |
| `packages/ui/src/*.ts` | Empty today; populate per Phase E |

### New (didn't exist)

| Component | Scope |
|---|---|
| `crates/akua-core/src/mod_file.rs` | `akua.mod` / `akua.sum` parser + resolver (go-mod shape) |
| `crates/akua-core/src/kcl_package.rs` | `Package.k` loader invoking embedded KCL interpreter via `kclvm-rs` |
| `crates/akua-core/src/policy.rs` | Embedded OPA evaluator, Rego module loader, compile-resolved import graph |
| `crates/akua-core/src/embedded/kustomize.rs` | Kustomize wasip1 embedding |
| `crates/akua-core/src/embedded/kro.rs` | kro offline instantiator |
| `crates/akua-core/src/embedded/regal.rs` | Regal linter embedding |
| `crates/akua-core/src/embedded/kyverno.rs` | Kyverno → Rego converter |
| `crates/akua-core/src/agent_context.rs` | Agent auto-detection ([`cli-contract.md §1.5`](./cli-contract.md)) |
| `crates/akua-dev/` (new crate) | Content-addressable build graph + file watcher + `localhost:5173` UI (Phase D) |
| `crates/akua-repl/` (new crate) | Interactive Rego + KCL REPL (Phase D) |

---

## 2. Agent-execution methodology

The rewrite is designed for agent-driven execution. Every phase decomposes into independently verifiable tasks an agent can pick up, execute, and exit on.

### Ground rules

1. **One task = one PR.** No mega-PRs. Each task passes `akua check && akua lint && akua test && akua fmt --check` on its own.
2. **Every task has a reference spec.** The agent's first action on any task is to read the linked spec section. No guessing shape.
3. **Every task has a verification step.** A specific example, a specific assertion, a specific rendered-output comparison. No "looks good to me."
4. **Agents follow CLAUDE.md invariants mechanically.** Violations are architectural bugs, not style issues.
5. **Policy-gated merges.** Even during the rewrite, `akua policy check` on the rewrite branch must pass for the tier we're bootstrapping.

### Task format (copy into GitHub issue / agent prompt)

```
Title: [Phase X.Y] <one-verb deliverable>

Spec reference: docs/<spec-file>.md §<section>
Carry-forward: crates/<path>.rs (reuse as-is)
Rewrite scope: crates/<path>.rs
New files: crates/<path>.rs (empty; follow spec)

Acceptance:
- Unit tests for <specific shape>
- Integration test: `akua <verb> examples/<sample>` produces <expected-bytes>
- `akua check && akua lint && akua test && akua fmt --check` passes
- CLAUDE.md invariants respected: no non-determinism, no YAML-as-truth, typed-code-canonical

Do not:
- Add non-K8s deploy targets
- Introduce Temporal or Cloud-side concerns (those live in a different repo)
- Rename existing public APIs beyond what the spec requires
```

### Sequence within a phase

Within each phase, tasks execute in a partially ordered DAG. Agents pick up any unblocked task. A task is unblocked when its dependencies' PRs are merged.

**Phase A dependency graph (example):**

```
  [A.1] akua.mod/sum parser        ──┐
                                     ├─▶  [A.4] CLI skeleton  ──▶  [A.7] examples/01 renders end-to-end
  [A.2] KCL Package loader         ──┤
                                     │
  [A.3] CLI contract primitives    ──┘
  (--json, --plan, typed exits,
   idempotency, agent detection)

  [A.5] akua render (KCL-only)     ──┐
                                     ├─▶  [A.8] @akua/sdk render parity
  [A.6] akua publish + verify      ──┘
```

A.1, A.2, A.3 are independent — three agents can pick them up in parallel. A.4 depends on all three. A.5, A.6 branch from A.4. A.7, A.8 are the phase exit gate.

---

## 3. Phase-by-phase task decomposition

Each task below is sized for a single agent session (~1–3 hours of focused work including tests).

### Phase A — Foundation

**A.1 — `akua.mod` + `akua.sum` parser**
- Spec: [`lockfile-format.md`](./lockfile-format.md)
- Deliverable: `crates/akua-core/src/mod_file.rs` — TOML parse, dep-form discrimination (oci/git/path/replace), workspace member resolution, `akua.sum` digest+signature ledger read/write.
- Tests: round-trip every example in the spec; reject malformed inputs with typed errors.
- No network calls in parser; digest resolution is a separate concern.

**A.2 — `Package.k` loader (KCL evaluation)**
- Spec: [`package-format.md`](./package-format.md)
- Deliverable: `crates/akua-core/src/kcl_package.rs` — wrap `kclvm-rs` crate; extract schema, resolve imports (against `akua.mod`), expose `Input` schema for inspection.
- Tests: `examples/01-hello-webapp/Package.k` parses, schema extractable, import graph resolves.
- Carry-forward: reuse `kcl-lang` git dep from v0.3.

**A.3 — CLI contract primitives**
- Spec: [`cli-contract.md`](./cli-contract.md) (§1 through §15)
- Deliverable: `crates/akua-cli/src/contract/` — `--json` / `--plan` / typed exit codes (0/1/2/3/4/5/6) / `--timeout` / `--idempotency-key` as a reusable argument-group + response-shaping layer. Agent context auto-detection per §1.5. Structured errors on stderr.
- Tests: every exit code reachable via a stub verb; `akua whoami --json` returns agent-context structure.

**A.4 — CLI skeleton wiring**
- Deliverable: 30 verbs registered in clap with stubbed handlers returning `exit 2 system-error: not-implemented`. Each handler reads CLI contract primitives from A.3.
- Tests: `akua help --json` returns the full verb tree; every verb accepts `--json` and `--plan`.
- Carry-forward: `crates/akua-cli/src/main.rs` structure.

**A.5 — `akua render` (KCL-only)**
- Spec: [`cli.md`](./cli.md) `render` section + [`package-format.md`](./package-format.md) output format
- Deliverable: execute a `Package.k` with given inputs, produce `resources[]` + `outputs[]` per the KCL program's declarations. KCL-only (no Helm, no Kustomize yet).
- Tests: `examples/01-hello-webapp` produces expected manifests; byte-identical across three runs.
- Depends on: A.2, A.4.

**A.6 — `akua publish` + `akua verify`**
- Carry-forward: `crates/akua-core/src/publish.rs` + `attest.rs`.
- Deliverable: wire existing OCI push + SLSA emission to the new verb surface. Consume `akua.sum` for reproducibility checks.
- Tests: round-trip publish + verify against local OCI registry (zot).
- Depends on: A.1, A.4.

**A.7 — End-to-end sample**
- Deliverable: `examples/01-hello-webapp/Package.k` + `examples/01-hello-webapp/App.k` + a `README.md` that walks author → render → publish → verify. Used as the exit gate.
- Depends on: A.5, A.6.

**A.8 — `@akua/sdk` render parity**
- Spec: [`sdk.md`](./sdk.md)
- Deliverable: `packages/sdk/src/render.ts` produces byte-identical output to `akua render` for the same inputs. Same for `publish` and `verify`.
- Tests: cross-consumer determinism test (CLI output hash == SDK output hash).
- Depends on: A.5, A.6.

**Phase A exit gate:** A.7 works end-to-end from CLI and SDK. All prior PRs merged.

### Phase B — Multi-engine pipeline

**B.1 — `helm.template()` as KCL callable**
- Carry-forward: `crates/helm-engine-wasm/` (no changes to engine itself).
- Deliverable: KCL plugin exposing `helm.template(chart, values)` that invokes the embedded Helm engine and returns a typed `[{kind, apiVersion, metadata, ...}]` list KCL can consume.
- Tests: a KCL Package that composes Helm + KCL output renders identically to hand-written raw manifests.

**B.2 — Kustomize embedding**
- Deliverable: `crates/akua-core/src/embedded/kustomize.rs` — Go→wasip1 via wasmtime, similar to helm-engine-wasm.
- KCL callable: `kustomize.build(base, overlays)`.

**B.3 — kro offline instantiator**
- Deliverable: `crates/akua-core/src/embedded/kro.rs`. kro's RGD instantiation done offline (no controller), producing the same YAML the controller would produce.
- KCL callable: `rgd.instantiate(rgd_def, instance_spec)`.

**B.4 — CEL callable from KCL**
- Carry-forward: existing `cel-interpreter` integration.
- KCL callable: `cel.eval(expr, ctx)`.

**B.5 — `akua diff`**
- Spec: [`cli.md`](./cli.md) `diff`.
- Deliverable: structural diff between two rendered outputs; stable, readable, parseable with `--json`.

**B.6 — `akua inspect`**
- Spec: [`cli.md`](./cli.md) `inspect`.
- Deliverable: full-tree output (schema, deps, attestations, metadata) for any OCI-published artifact.

**B.7 — Input validation against source schemas**
- When `helm.template()` is called with a chart that ships `values.schema.json`, validate resolved values at render time. Typos fail as lint errors, not Helm template failures.

**Phase B exit gate:** `examples/02-webapp-postgres` (CNPG + webapp) renders end-to-end with byte-identical output; mixed-engine.

### Phase C — Policy engine

**C.1 — Embedded OPA evaluator**
- Deliverable: `crates/akua-core/src/policy.rs` — OPA evaluator linked as Rust crate (or wasip1 if needed).
- Tests: evaluate a sample Rego module against input data; capture verdict.

**C.2 — Compile-resolved policy imports**
- Spec: [`policy-format.md`](./policy-format.md)
- Deliverable: `akua.mod` imports like `tier-prod = { oci = "oci://policies.akua.dev/tier/production", version = "1.2.0" }` resolve at compile time; Rego modules mount as `data.akua.policies.tier.production`. Never runtime string lookups.

**C.3 — `akua policy check`**
- Deliverable: verdict path returns `{allow | deny | needs-approval}` + structured reasons.

**C.4 — `akua test` (Rego + KCL)**
- Deliverable: run `*_test.rego` via embedded OPA test runner; run `test_*.k` via embedded KCL test harness.
- Spec: [`cli.md`](./cli.md) `test`.

**C.5 — Kyverno-to-Rego converter**
- Deliverable: `crates/akua-core/src/embedded/kyverno.rs` — take a Kyverno ClusterPolicy YAML, emit equivalent Rego. Consumed at compile time via `akua.mod`.

**C.6 — Policy tier publishing**
- Deliverable: `tier/dev`, `tier/startup`, `tier/production`, `tier/audit-ready` authored in Rego, published as signed OCI artifacts under `oci://policies.akua.dev/...`.

**C.7 — policy composition convention**
- Deliverable: document the conventional shape of a workspace's Rego layout — local `.rego` files in `./policies/`, tiers imported via `akua.mod` as compile-resolved `data.*`. No akua-specified `PolicySet` kind; users compose Rego files freely.

**Phase C exit gate:** a PR on a test workspace that violates `tier/production` is blocked with a line-precise deny verdict.

### Phase D — Deploy + dev loop

**D.1 — `akua deploy` reconciler drivers**
- Deliverable: `--to=argocd`, `--to=flux`, `--to=kro`, `--to=helm`, `--to=kubectl`, `--to=<custom>`. No non-K8s drivers.
- Each driver: emit the reconciler's native consumable; apply or commit as appropriate.

**D.2 — `akua dev` build graph**
- Deliverable: `crates/akua-dev/` — content-addressable build DAG, `notify-rs` watcher, change classifier, incremental rebuild.

**D.3 — `akua dev` browser UI**
- Deliverable: WebSocket-driven UI at `localhost:5173` showing pipeline stages, resource health, log tail, manifest diff. Terminal fallback via Ratatui.

**D.4 — `akua dev` local target**
- Deliverable: kind / k3d / minikube integration; server-side apply; persistence across restarts; `*.127.0.0.1.nip.io` default DNS.

**D.5 — `akua repl`**
- Deliverable: interactive Rego + KCL REPL. Command-history, tab-complete, `--json` out mode for agent consumption.

**D.6 — `akua trace` + `akua cov`**
- Deliverable: policy evaluation trace (partial-eval tree); coverage report for Rego test suite.

**D.7 — `akua query`**
- Deliverable: Loki / Prom queries dispatched from the CLI against configured cluster endpoints. No federation in v1.

**Phase D exit gate:** solo-developer journey completes on a fresh laptop in under 5 minutes; `akua dev` edit-to-applied loop under 500ms median.

### Phase E — Browser playground + Studio

**E.1 — `akua.dev` playground**
- Deliverable: upload / paste a `Package.k`, live render (via WASM), live diff against prior version, live lint. Public.

**E.2 — `@akua/ui` components**
- Deliverable: PackageEditor (Monaco + KCL LSP), FormPreview (derived from `@ui` decorators on KCL schemas), ManifestViewer (YAML diff), TestRunner (live results).

**E.3 — Review surface template**
- Deliverable: open-source UI component library that renders a review bundle (structural diff, rendered diff, policy verdict, attestation chain). Hosted version lives elsewhere.

**Phase E exit gate:** a public visitor authors a working Package at `akua.dev`, renders, checks policy, sees structural diff — all without installing anything.

---

## 4. How to invite an agent into the rewrite

The repository ships agent skills ([`skills/`](./skills/)) following the [Agent Skills Specification](https://agentskills.io/specification). An agent cloning this repo:

1. Reads CLAUDE.md at the repo root.
2. Reads the phase-current task (see GitHub Issues with label `phase-A` / `phase-B` / ...).
3. Opens the linked spec section.
4. Writes code + tests matching the acceptance criteria.
5. Runs `akua check && akua lint && akua test && akua fmt --check`.
6. Opens a PR with the task title.

If the agent is blocked on a design decision, it opens an issue with label `design-question` rather than guessing. The masterplan (internal) decides; the answer lands as a spec update; the agent picks up the task again.

Skills like [`new-package`](./skills/new-package/), [`test-and-lint`](./skills/test-and-lint/), [`publish-signed`](./skills/publish-signed/), [`apply-policy-tier`](./skills/apply-policy-tier/) cover the common workflows. A skill for `rewrite-verb <verb-name>` is worth adding early in Phase A to standardize how agents pick up verb-rewrite tasks.

---

## 5. Verification across the rewrite

Every phase's exit gate includes one of the existing examples in [`examples/`](./examples/). The examples are the ground truth: if an example stops working, a phase is not complete.

Additional cross-cutting checks:

- **Determinism.** `akua render` on any example, run three times, produces byte-identical output. Run in CI.
- **CLI / SDK parity.** Every verb that produces output is called from both `akua <verb>` and `@akua/sdk.<verb>()`; outputs must match byte-for-byte. Run in CI.
- **Agent contract.** `akua whoami --json` exposes the agent context correctly under `CLAUDECODE=1` / `CURSOR_CLI=1` / `GEMINI_CLI=1` / `AGENT=foo`. CI matrix runs verbs under each.
- **Policy gate.** The rewrite branch maintains a passing `akua policy check` against `tier/production`. Merges to main require green.

---

## 6. Known risks

- **KCL workspace ergonomics.** kpm doesn't yet fully support workspace-level dep resolution. Phase A may need to contribute upstream or build a thin workaround.
- **Embedded engine sizes.** Helm's wasip1 build is ~75 MB. Target binary size with all engines embedded is ~85 MB. Acceptable for pre-alpha; may need size optimization pass pre-GA.
- **Agent drift.** Agents implementing tasks may introduce inconsistent patterns across crates. Mitigation: `apply-policy-tier` skill enforces the CLI contract mechanically; code review catches the rest.
- **Sample coverage.** The three existing examples cover common cases but not all. Phase B should add a kro-centric sample; Phase C should add a policy-centric sample.

---

## 7. Out of scope for this plan

The Cloud-side coordination layer (akua Cloud): GitHub App, Worker proxy, Convex schemas, Temporal workflow definitions, gatekeeper transaction, review UI backend, agent sandbox orchestration. Those live in a separate repo and are not OSS. This plan covers only the OSS substrate the CLI, SDK, and browser playground consume.

See the masterplan (private, internal) for the Cloud-side phasing.
