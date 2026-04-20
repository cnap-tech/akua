# CLAUDE.md

Ambient context for Claude Code and other agents working in the akua repo. If a change doesn't sit well against these invariants, it's wrong — regardless of whether it looks correct.

## What akua is

akua is the **bun/deno pattern applied to cloud-native infrastructure**. One binary (`akua`). Every verb. Single coherent toolkit for the whole packaging + platform lifecycle:

| bun / deno has | akua has |
|---|---|
| package manager | `akua add` / `akua pull` / `akua publish` + `akua.mod` + `akua.sum` |
| runtime (executes your program) | `akua render` (executes a Package's KCL + engine calls) |
| test runner | `akua test` (*_test.rego + test_*.k, golden tests) |
| formatter | `akua fmt` (.k + .rego) |
| linter | `akua lint` (Regal + kcl lint + cross-engine) |
| REPL | `akua repl` (Rego / KCL) |
| dev loop | `akua dev` (sub-second hot reload) |
| bundler / ship | `akua publish` (signed OCI + SLSA attestation) |

Plus: deploy driver (`akua deploy`), observability query (`akua query`), policy engine host (`akua policy`, Rego-native), infra primitives (`akua infra`), audit spine (`akua audit`). Thirty verbs, one contract, one mental model.

**Primary user:** AI agents operating in Linux sandboxes. Humans at policy-gated checkpoints. See [docs/agent-usage.md](docs/agent-usage.md).

**Positioning:** substrate, not content. We ship the pipeline, the format, the signing, the verification. The ecosystem ships packages, policies, and patterns.

## Before making any change

1. Check if a skill already covers the task: [`skills/`](skills/)
2. Read the relevant format spec in [`docs/`](docs/)
3. Obey [`docs/cli-contract.md`](docs/cli-contract.md) — every verb
4. Run `akua check && akua lint && akua test && akua fmt --check` locally
5. If touching policy: `akua policy check` verdict must be `allow`

## Canonical reference map

| concern | authoritative doc |
|---|---|
| CLI verbs + flags + exit codes | [docs/cli.md](docs/cli.md) |
| CLI invariants (universal contract) | [docs/cli-contract.md](docs/cli-contract.md) |
| Package authoring shape | [docs/package-format.md](docs/package-format.md) |
| Policy authoring shape | [docs/policy-format.md](docs/policy-format.md) |
| KRM kinds + YAML view | [docs/krm-vocabulary.md](docs/krm-vocabulary.md) |
| `akua.mod` + `akua.sum` | [docs/lockfile-format.md](docs/lockfile-format.md) |
| Embedded engines (KCL / Helm / OPA / Regal / Kyverno / CEL / Kustomize) | [docs/embedded-engines.md](docs/embedded-engines.md) |
| Agent install + skill format | [docs/agent-usage.md](docs/agent-usage.md) |
| TypeScript SDK | [docs/sdk.md](docs/sdk.md) |
| Roadmap + phasing | [docs/roadmap.md](docs/roadmap.md) |
| Implementation plan (agent-driven) | [docs/impl-plan.md](docs/impl-plan.md) |
| Runnable examples | [examples/](examples/) |
| Agent-executable workflows | [skills/](skills/) |
| Strategic spine (internal, maintainers) | `../cortex/workspaces/robin/akua-masterplan.md` |

## Non-negotiable invariants

Violations of these are architectural bugs:

**CLI contract holds universally.** Every verb emits `--json`, supports `--plan`, uses typed exit codes (0/1/2/3/4/5/6), accepts `--timeout` and `--idempotency-key` on writes. Structured errors on stderr, never prose. Agent-context auto-detection runs silently; see [cli-contract §1.5](docs/cli-contract.md#15-agent-context-auto-detection).

**Canonical form is typed code.** KCL for Packages + cluster-facing KRMs (App, Environment, Cluster, Secret, SecretStore, Gateway). Rego for Policies. YAML is a derived view via `akua export`, never authoritative. Never author YAML and treat it as source of truth for these.

**Determinism is load-bearing.** No `now()`, no `random()`, no env reads, no filesystem reads, no cluster reads inside the render pipeline. Same inputs + same lockfile + same akua version → byte-identical output.

**Signed + attested by default.** `akua publish` emits cosign signature + SLSA v1 predicate unless the caller explicitly opts out. Consumers verify by default on pull.

**`akua render` ≠ `akua export`.** `render` executes the Package's program (invokes engines, produces deploy-ready manifests). `export` converts a canonical artifact to a format view (JSON Schema, OpenAPI, YAML, Rego bundle). They are different verbs for different jobs.

## Architecture discipline

- **Substrate, not content.** We do not curate a package catalog. Upstream projects publish their own signed packages; akua provides signing + distribution + diff + audit infrastructure. Same logic for policy: Rego is a host, not a DSL we own.
- **External engines as compile-resolved imports or callable functions.** `helm.template(...)`, `rgd.instantiate(...)`, `kustomize.build(...)` are KCL callables. Kyverno / CEL / foreign Rego are `import data.…` in Rego, resolved via `akua.mod`. Never runtime string lookups like `kyverno.check({bundle: "oci://..."})`.
- **Embedded by default.** KCL, Helm, OPA, Regal, Kustomize, kro offline instantiator, CEL, Kyverno-to-Rego converter are all bundled into the akua binary via wasmtime (Rust engines linked directly). `$PATH` never required. Shell-out available as escape hatch via `--engine=shell`.
- **Compose with the ecosystem, don't replace it.** ArgoCD, Flux, kro, Helm release lifecycle, kubectl, Crossplane are first-class consumers of akua output. We target their formats (`RawManifests`, `HelmChart`, `ResourceGraphDefinition`, `Crossplane`, `OCIBundle`). We don't ask customers to switch reconcilers.

## KRM split

- **Cluster-facing** (full `apiVersion/kind/metadata` envelope; KCL canonical, YAML view generated): `App`, `Environment`, `Cluster`, `Secret`, `SecretStore`, `Gateway`.
- **Control-plane** (typed KCL only; YAML for interchange, not canonical): `Package`, `Policy`, `Rollout`, `Runbook`, `Budget`, `Incident`, `Experiment`, `Tenant`.

## What we refuse

- Marketing-speak ("empower," "democratize," "unlock," "revolutionize," "AI-first"). Describe what things do; don't frame.
- Emoji in files (unless explicitly requested by a human user).
- Feature-bingo changelogs. Ship refinements, not bullets.
- Settings panels for things with an obvious correct default. Pick a default and defend it.
- Inventing akua-specific vocabulary when a standard exists (JSON Schema, OpenAPI, KRM, OCI, cosign, SLSA, Agent Skills Specification).
- Vendor lock-in without escape hatches. Every artifact exports to standards; every user can leave.
- YAML-typed-everywhere. Typed composition demands KCL; raw values can be YAML; don't confuse layers.

## Quality gates

```sh
akua check          # fast syntax / type / dep check, no execution
akua fmt --check    # fail CI if any file needs formatting
akua lint           # Regal + kcl lint + cross-engine
akua test           # unit tests (*_test.rego, test_*.k) + golden
akua verify         # akua.mod ↔ akua.sum integrity + cosign
akua policy check   # when policy changed; verdict must be allow
```

All are embedded — no installation of `opa` / `kcl` / `regal` needed.

## Development stance

- The project is **pre-alpha** (v0.x). APIs churn. Every change should be reviewed against the invariants above before shipping.
- **Always verify doc freshness.** If you're about to quote a spec, open the file; the design has iterated.
- **Never defeat determinism "temporarily."** If you need non-determinism for a reason, it's a design bug in the calling code.
- **When uncertain, ask.** Don't guess at contract shape. The specs are precise for a reason.

## How to add a skill

Skills live under [`skills/<skill-name>/SKILL.md`](skills/) and follow the [Agent Skills Specification](https://agentskills.io/specification):

```
---
name: skill-name              # lowercase, hyphens, matches directory
description: 1–1024 chars; include trigger keywords
---

# Title

Step-by-step instructions...
```

Validate: `npx skills-ref validate ./skills/<name>`. Skills ship with the repo and install across Claude Code / Cursor / Codex / Gemini CLI / Goose / Amp / OpenCode / Cline / 25+ others (see [docs/agent-usage.md](docs/agent-usage.md)).

## The one-sentence discipline

Everything in akua follows one rule: **typed, signed, deterministic state lives in git; tools compose through shell; renderers target any substrate.** When a proposed change reads well against that rule, it's probably right. When it doesn't, it probably isn't.
