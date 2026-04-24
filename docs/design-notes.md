# akua Design Notes

> **Scope.** *Why* akua is shaped the way it is — positioning, invariants, trade-offs that keep being re-discussed. Read this before proposing architectural changes.
>
> **Authoritative specs live elsewhere:**
> - CLI shape → [`cli.md`](./cli.md) + [`cli-contract.md`](./cli-contract.md)
> - Package authoring → [`package-format.md`](./package-format.md)
> - Policy authoring → [`policy-format.md`](./policy-format.md)
> - Lockfile → [`lockfile-format.md`](./lockfile-format.md)
> - Engines → [`embedded-engines.md`](./embedded-engines.md)
> - SDK → [`sdk.md`](./sdk.md)
>
> This doc carries the *reasoning*. Those docs carry the *contracts*.

---

## 1. Positioning

### What akua is

- **A single binary and SDK** covering the whole packaging + platform lifecycle: author, render, test, lint, format, sign, publish, verify, diff, inspect, deploy, query, audit. Thirty verbs. One mental model. The bun/deno pattern applied to cloud-native.
- **Typed composition.** Packages authored in **KCL** with first-class schemas; policies authored in **Rego**. YAML is a derived view via `akua export`, never authoritative.
- **Deterministic transformation.** Same inputs + same lockfile + same akua version → byte-identical output. No non-determinism inside the render pipeline.
- **Signed + attested by default.** cosign signature + SLSA v1 predicate on every `akua publish`. Consumers verify by default on pull.
- **Agent-first.** The primary user is an AI agent operating in a Linux sandbox. Humans gate at policy checkpoints. See [`agent-usage.md`](./agent-usage.md).

### What akua is *not*

- **Not a reconciler.** ArgoCD / Flux / kro / kubectl own cluster-side reconciliation. We hydrate; they apply.
- **Not a Kubernetes control plane.** No controllers, no operator-pattern state machines.
- **Not a Helm competitor.** We embed Helm as one engine among several. Kustomize, KCL, kro, Crossplane — all first-class, all interoperable through the same pipeline.
- **Not a non-Kubernetes deploy target.** We emit formats Kubernetes-ecosystem reconcilers consume. Fly / Workers / Lambda are out of scope.
- **Not a curated package catalog.** Upstream projects publish their own signed packages; we ship the substrate.

---

## 2. The load-bearing invariants

Violations of these are architectural bugs, regardless of how correct the change looks locally.

### 2.1 Canonical form is typed code

KCL for Packages. Rego for Policies. `Package.k` is the one akua-specified shape; everything higher-level (App, Environment, Cluster, Secret, Gateway, Workspace, PolicySet, Rollout, Runbook, Budget, Incident, …) is user territory — authored as user-defined KCL schemas inside the workspace. akua does not own that vocabulary.

YAML is interchange; it is never authoritative. Authoring KCL type-checks before render runs. Every category error caught at author-time is an incident prevented at runtime.

### 2.2 Determinism is load-bearing

No `now()`, no `random()`, no env reads, no filesystem reads, no cluster reads inside the render pipeline. Render is a pure function of `(inputs, lockfile, akua-version)`.

Consequences:
- Cross-environment reproducibility (laptop = CI = browser playground).
- Content-addressable caching actually works (hash in → bytes out).
- Reviewers can trust that a diff shown in CI is what will deploy.
- Rollback is trivial — the prior lockfile regenerates prior bytes.

If you need non-determinism for a reason, it's a design bug in the calling code. There is no escape hatch inside the render pipeline.

### 2.3 External engines are compile-resolved, not runtime-looked-up

Helm, kro RGDs, kustomize, jsonnet are **KCL callable functions** (`helm.template(...)`, `rgd.instantiate(...)`, `kustomize.build(...)`). Kyverno / CEL / foreign Rego modules are `import data.…` in Rego, resolved via `akua.toml`.

Never runtime string lookups like `kyverno.check({bundle: "oci://..."})`. Runtime lookup means the evaluator has to trust an external network at render time. Compile-resolved imports keep determinism + signature verification + offline operation.

### 2.4 Embedded engines by default

KCL, Helm, OPA, Regal, Kustomize, kro offline instantiator, CEL, Kyverno-to-Rego converter — all bundled into the akua binary via wasmtime (Rust engines linked directly; Go engines compiled to wasip1). `$PATH` is never required and there's no shell-out escape hatch — CLAUDE.md's "No shell-out, ever" invariant holds across the render pipeline.

Two consequences: (a) `akua` works in any sandbox without pre-provisioning, (b) version drift between engines is impossible — we ship a tested set together.

### 2.5 `akua render` ≠ `akua export`

`render` executes the Package's program (invokes engines, produces deploy-ready manifests). `export` converts a canonical artifact to a format view (JSON Schema, OpenAPI, YAML, Rego bundle). Different verbs for different jobs. Conflating them is the most common interface mistake; the CLI contract keeps them separate.

### 2.6 Compose with the ecosystem, don't replace it

ArgoCD, Flux, kro, Helm release lifecycle, kubectl, Crossplane are first-class consumers of akua output. We target their formats (`RawManifests`, `HelmChart`, `ResourceGraphDefinition`, `Crossplane`, `OCIBundle`). We don't ask customers to switch reconcilers.

---

## 3. Trade-offs worth naming

### 3.1 KCL over bespoke DSL

We considered inventing an akua-specific config language. Rejected: KCL is already typed, has a real LSP, has schemas, runs offline, has a mature ecosystem, and is mechanically similar enough to Python/TypeScript that onboarding takes hours, not weeks.

Consequence: we inherit KCL's choices (including its not-quite-Python syntax) and its evolution cadence. Mitigation: we contribute upstream when high-leverage (digest-pinning lockfile, for example) and build on top for the rest.

### 3.2 Rego as policy host

Rego is awkward to learn but genuinely solves cross-resource reasoning and partial evaluation — jobs you cannot do cleanly in KCL. Kyverno is k8s-scoped; CEL can't express cross-resource rules; OPA with Rego is the mature choice.

We make Rego palatable through tooling (Regal linter, opa test runner, `akua repl`) and compile-resolved imports so custom rules stay small.

### 3.3 OCI registries, not a central catalog

Centralized package curation is fragile (see Bitnami's deprecation and its fallout). First-party publishing is the durable pattern. We ship signing + distribution + diff + audit infrastructure so any maintainer can publish trustworthy packages themselves.

Consequence: no shelf to browse on day one. Mitigation: browser-based audit at `akua.dev` for any public artifact; `akua init` templates for common starts.

### 3.4 Helm v4 still an engine

Helm's template language is widely hated but widely adopted. The sizing charts, the community patterns, the ISV-published charts — replacing this ecosystem would take a decade. Embedding it as one engine among several gets us the adoption path without endorsing the ergonomic.

See [`embedded-engines.md`](./embedded-engines.md) for the forked-Helm rationale (client-go stripped, wasip1 target).

---

## 4. The one-sentence discipline

Everything in akua follows one rule: **typed, signed, deterministic state lives in git; tools compose through shell; renderers target any substrate.** When a proposed change reads well against that rule, it's probably right. When it doesn't, it probably isn't.

---

## 5. Engine determinism reality check

The ambition is "all engines run as embedded, deterministic, browser-portable WASM." Reality after ecosystem survey:

| Engine | Deterministic | Browser-portable | Embedded | Notes |
|---|---|---|---|---|
| KCL | ✅ | ✅ (via `kcl-lang/wasm-lib`) | ✅ (`kclvm-rs`) | |
| Helm v4 | ✅ | ⚠️ (big binary, Go→wasip1 works) | ✅ (forked, ~75 MB) | size optimization pending |
| OPA / Rego | ✅ | ✅ | ✅ | |
| Regal (lint) | ✅ | ✅ | ✅ | |
| Kustomize | ✅ | ⚠️ (Go→wasip1) | ✅ | |
| kro RGD instantiator | ✅ | ✅ | ✅ (offline-mode fork) | |
| CEL | ✅ | ✅ | ✅ | |
| Kyverno | ✅ | ✅ (via Rego converter) | ✅ | consumed as Rego via converter |
| Helmfile | ❌ | ❌ | ❌ | does not ship — shell-required, not embeddable; compose `helm.template(...)` calls in a Package instead |

Practical guidance: Helmfile breaks every invariant (non-deterministic, shell-required, not embedded) and therefore does not ship. Migrate Helmfile workflows to akua Packages that compose `helm.template(...)` calls instead.

---

## 6. Open questions

See the masterplan §18 for the authoritative open-questions list. The ones specific to the OSS substrate:

1. **KCL workspace ergonomics.** kpm doesn't yet support workspace-level dep resolution the way cargo does. Do we contribute upstream, build on top, or accept the gap until the community solves it?
2. **Engine plugin distribution.** Extism convention is OCI; we agree. Concrete schema + signing flow still to spec.
3. **Cross-package composition.** Package A imports Package B — umbrella-of-umbrellas is straightforward; API for referencing another package by OCI digest is not fully specced yet.
4. **Chart signing completeness.** cosign is consensus; do we sign the chart itself *and* the SLSA attestation, or only the attestation? Probably both, to be specified in `cli-contract.md`.

---

## Glossary

- **Package** — authored unit (KCL `Package.k` + engine sources). Reusable; published to OCI.
- **App** — per-install document (user-defined schema) referencing a Package by OCI digest; carries the customer's values.
- **Chart** — Helm's unit of distribution (`.tgz` + `Chart.yaml`). Akua consumes and emits these.
- **Install** — one deployed instance of a Package in a specific environment.
- **Source** — one component inside a Package (a chart, a KCL dir, a kustomize base).
- **Engine** — the tool that turns a source into a chart fragment (helm, kcl, kustomize, rgd, kyverno→rego, ...).
- **Umbrella** — a top-level Chart.yaml that aliases all sources in a multi-engine Package.
- **`akua.toml`** — human-edited manifest of declared deps (go-mod shape).
- **`akua.lock`** — machine-maintained ledger of resolved digests + cosign signatures.
- **Policy tier** — signed OCI-distributed Rego bundle (`tier/dev`, `tier/production`, `tier/audit-ready`).
