# Policy format

akua's policy architecture mirrors its packaging architecture. One host language chosen for what it's actually good at, with external specialized engines available as compile-resolved imports.

| | packaging | policy |
|---|---|---|
| Host language | KCL | **Rego** |
| Sandboxed, non-Turing-complete | ✓ | ✓ |
| Composition via | `import` + schema inheritance | `import` + rule merging |
| External engines | `helm.template`, `rgd.instantiate`, `kustomize.build` | Kyverno, CEL, KCL `check:`, custom |
| External engine form | KCL callable functions | **compile-resolved `import data.…`** |
| Package manager | `akua.mod` + `akua.sum` | `akua.mod` + `akua.sum` (same file) |
| Runtime | `kclvm` (WASM) | embedded OPA |

Policy in akua is always Rego at evaluation time. Everything else is either a schema-level `check:` block in KCL (caught during package render) or a foreign artifact compiled to Rego at build time.

---

## 1. Why Rego

- **Designed for the job.** Datalog-derived constraint evaluation; set comprehensions, aggregation, negation-as-failure, partial evaluation — all native.
- **Ecosystem.** OPA is CNCF-graduated. Thousands of existing policies in the OPA Policy Library, Kyverno community, Styra DAS, Conftest. Compliance teams know it.
- **Performance.** Partial evaluation compiles rules to Wasm or decision tables for sub-millisecond evaluation.
- **Interop.** Customers with existing Gatekeeper / Kyverno / Conftest deployments reuse their investment.

See [cli-contract.md](cli-contract.md) for the higher-level discussion of why not a custom DSL and why not "KCL for everything."

---

## 2. Two-layer model

Policy in akua is split across two layers with different tools:

| layer | tool | catches | when |
|---|---|---|---|
| **Schema validation** | KCL `check:` blocks in Package | well-formedness of a single resource: types, required fields, field-local constraints | render time, in-process |
| **Policy evaluation** | Rego | cross-resource rules, aggregation, compliance constraints | render time + deploy time + admission time |

They're not competitors. Authors writing a Package define `check:` blocks inline; policy tiers composed by platform teams live in Rego files. Same package's render pipeline runs both.

---

## 3. The Rego host

A workspace's policy bundle is one or more `.rego` files, typically under `policies/`. akua does not specify a `PolicySet` kind — composition is just Rego file layout, imports, and rule merging.

```rego
# policies/production.rego
package akua.policies.my_org_production

import future.keywords

# Compile-resolved imports (pinned in akua.mod, verified via akua.sum)
import data.akua.policies.tier.production
import data.akua.policies.kyverno.security
import data.akua.policies.cel.my_expressions

# Inherit rules from imported tiers and bundles
deny[msg] { tier.production.deny[msg] }
deny[msg] { kyverno.security.deny[msg] }
deny[msg] { cel.my_expressions.deny[msg] }

# Local rule — pure Rego
deny[msg] {
    input.resource.kind == "Deployment"
    not input.resource.metadata.labels["team"]
    msg := "production Deployments must have a team label"
}

# Cross-resource aggregation — Rego's sweet spot
deny[msg] {
    total_cpu := sum([r.spec.resources.requests.cpu |
                      r := input.resources[_];
                      r.kind == "Deployment"])
    total_cpu > input.environment.budget.cpu
    msg := sprintf("total CPU %d exceeds environment budget %d",
                   [total_cpu, input.environment.budget.cpu])
}

# Call an akua runtime builtin — only for things that genuinely need runtime context
deny[msg] {
    pkg := akua.package(input.resource.metadata.annotations["akua.dev/package"])
    pkg.schema.version < "2.0"
    msg := sprintf("package %s is at schema v1; production requires v2+", [pkg.ref])
}
```

Rules:

- Package name follows reverse-DNS: `akua.policies.<org>.<name>`
- Rules accumulate via Rego's natural merging: multiple files in the same package contribute to the same `deny` set
- Imports are compile-resolved; runtime lookup by string is disallowed
- Every rule produces a `msg` with a clear, actionable explanation

---

## 4. External engines as compile-resolved imports

Three strategies for bringing non-Rego policy content in:

### 4.1 Foreign Rego modules

Another Rego policy bundle. Direct import; no conversion.

```toml
# akua.mod
[dependencies]
tier-prod = { oci = "oci://policies.akua.dev/tier/production", version = "1.2.0" }
```

```rego
import data.akua.policies.tier.production
deny[msg] { tier.production.deny[msg] }
```

### 4.2 Kyverno bundles (converted to Rego at build time)

Kyverno ships policies as Kubernetes CRDs with a native YAML DSL. akua's `akua add policy <kyverno-ref>` uses an embedded Kyverno→Rego converter to compile the bundle into Rego modules stored under `./.akua/policies/vendor/`.

```toml
# akua.mod
[dependencies]
kyverno-security = { oci = "oci://policies.akua.dev/kyverno/security", version = "2.0.0" }
```

```rego
import data.akua.policies.kyverno.security
deny[msg] { kyverno.security.deny[msg] }   # Kyverno rules, now evaluated as Rego
```

The conversion is one-way and happens at `akua add` time; the original Kyverno source is preserved for audit but not consumed at eval time. Reproducibility: same Kyverno version + same converter version → same Rego output.

### 4.3 CEL expression libraries (compiled to Rego)

CEL (Google's Common Expression Language) is simple enough to compile directly to Rego primitives. `akua add policy <cel-ref>` runs the CEL→Rego compiler; imported the same way:

```rego
import data.akua.policies.cel.my_expressions
deny[msg] { cel.my_expressions.deny[msg] }
```

Useful for reusing existing Kubernetes admission CEL (from `ValidatingAdmissionPolicy`) inside akua's policy framework.

### 4.4 KCL `check:` blocks as policy

`check:` blocks in KCL schemas are the schema-validation layer (§2). They don't import into Rego. They run during package render. If a customer wants to lift a schema check to a cross-resource rule, they rewrite it in Rego.

---

## 5. Custom runtime builtins

Only things that genuinely require runtime context — not static rules — live as custom OPA builtins akua provides:

| builtin | returns | use when |
|---|---|---|
| `akua.package(ref)` | package metadata + attestation | checking package version / signature at eval time |
| `akua.cluster(query)` | live cluster state (read-only, sandboxed) | live-mode policy only; never in CI policy |
| `akua.attestation(ref)` | signature chain + SLSA predicate | verifying provenance against current transparency log |
| `akua.diff(a, b)` | structural diff between package versions | upgrade-gate policies |
| `akua.env(path)` | fields from the workspace's Environment-shaped input (user-defined schema) | environment-specific rules |

Implementation: each builtin is Go code registered with OPA at akua process start. The set is small (~8 builtins), stable, and documented. Customers can add their own by writing a Go plugin (advanced; most won't).

**Anti-pattern avoided:** `kyverno.check({bundle: "oci://..."})` style runtime lookups. Policies should be compile-resolved imports, not runtime string dereferences. Runtime builtins are only for context that fundamentally cannot be known at compile time (live cluster state, current attestations, etc.).

---

## 6. Policy tiers

A **tier** is a Rego package distributed as a signed OCI artifact. Tiers compose via `import`; customers fork by forking a Rego package.

akua ships these reference tiers (small starter set, community / vendor content extends them):

- `tier/dev` — permissive defaults for local development
- `tier/startup` — basic resource limits, non-privileged, readiness probes
- `tier/production` — adds anti-affinity, budget caps, required approvals, soak gates
- `tier/audit-ready` — adds audit retention, access logging, change tracking. *Not a compliance certification by itself; helps produce evidence an auditor can verify.*

Tiers are just Rego packages. Composition:

```rego
# my-org extends tier/production with their own rules
package akua.policies.my_org

import future.keywords
import data.akua.policies.tier.production

deny[msg] { tier.production.deny[msg] }    # inherit

deny[msg] {                                  # add
    input.resource.metadata.labels["org"] != "acme"
    msg := "all production resources must carry org=acme label"
}
```

Override semantics are Rego's natural rule semantics: add a new rule with the same name in a later package to contribute; use `default` rules for fallback logic.

**What `tier/soc2` / `tier/hipaa` / `tier/fedramp-moderate` are NOT:** they are not compliance certifications. SOC2/HIPAA/FedRAMP are audits. A tier helps an auditor verify technical controls quickly; the audit itself is between the customer and their auditor. We call this layer `tier/audit-ready` plus targeted sub-bundles (`tier/audit-ready/access-logging`, `tier/audit-ready/retention-90d`) rather than claiming compliance-as-a-bundle.

---

## 7. Evaluation model

`akua policy check` runs against rendered resources:

```
(rendered manifests) + (Rego policy set) + (context) → verdict
```

Input to Rego:

```json
{
  "resource":     { ... one Kubernetes resource ... },
  "resources":    [ ... all resources in the render ... ],
  "environment":  { ... user-defined environment shape ... },
  "package":      { ... Package metadata ... },
  "team":         { ... Team context if available ... }
}
```

Rego evaluates all `deny` rules; produces a list of verdicts; akua aggregates into a final decision:

```json
{
  "tier": "my-org",
  "verdict": "allow" | "deny" | "needs-approval",
  "failing": [
    {
      "rule": "deny_budget_cap",
      "resource": "Deployment/api",
      "reason": "total CPU exceeds environment budget",
      "suggested_fix": "reduce replicas or request budget increase"
    }
  ],
  "approvers": ["@team/platform"]
}
```

`needs-approval` is determined by tier-defined `approve_required` rules; it pairs with `akua review` for human-in-the-loop gating.

---

## 8. Where policy fires

Policy evaluates at multiple points; each runs the same Rego against different inputs:

| point | input | failure mode |
|---|---|---|
| `akua render` / `akua dev` | rendered manifests + live context | lint error; render succeeds but marks the output as deny-policy |
| `akua deploy` / CI gate | rendered manifests + target environment | exit 3 (policy deny) or exit 5 (needs approval) |
| in-cluster admission (optional) | admission webhook payload | reject apply |
| audit sweep (scheduled) | current cluster state | produce an Incident record |

All four share the same Rego bundle. The host language guarantees uniform behavior across gates.

---

## 9. Authoring workflow

### New tier from scratch

```sh
akua init policy my-org-production
# creates policies/my-org-production.rego with starter template
```

### Import an existing tier

```sh
akua add policy oci://policies.akua.dev/tier/production --version 1.2.0
# adds to akua.mod + akua.sum; makes 'data.akua.policies.tier.production' importable
```

### Import a Kyverno bundle

```sh
akua add policy oci://policies.akua.dev/kyverno/security --version 2.0.0
# fetches Kyverno YAML, converts to Rego, stores under .akua/policies/vendor/
```

### Test a policy

```sh
akua policy check --tier my-org-production --target ./deploy/production
# runs Rego against rendered manifests; exit 0 / 3 / 5 per verdict
```

### Publish a tier

```sh
akua publish --policy my-org-production --to oci://policies.acme.com/my-org-production --tag v1.0.0
# pushes the Rego bundle signed + SLSA-attested
```

---

## 10. What Rego does NOT own

- **Schema validation.** Stays in KCL `check:` blocks at package authoring time. Rego cross-resource rules are separate.
- **Rendering logic.** `helm.template()` calls are in KCL. Rego doesn't participate in render.
- **Config generation.** Rego outputs decisions, not configuration. If a rule suggests a fix, that fix is a message (text); authors still do the edit.
- **Runtime config discovery.** KCL can't read env vars; Rego policies also can't (except via the small set of akua runtime builtins in §5).

---

## 11. Testing, linting, tracing

Policies without tests are liabilities. akua embeds OPA's full testing and debugging surface so authors work with the same ergonomics as the OPA CLI, without requiring OPA on `$PATH`.

### Writing tests

Rego test files are `*_test.rego`. Each test is a rule beginning with `test_`:

```rego
# policies/production_test.rego
package akua.policies.my_org_production

import future.keywords

test_deny_missing_team_label {
    deny["production Deployments must have a team label"] with input as {
        "resource": {
            "kind": "Deployment",
            "metadata": {"name": "api", "labels": {}}
        }
    }
}

test_allow_with_team_label {
    count(deny) == 0 with input as {
        "resource": {
            "kind": "Deployment",
            "metadata": {"name": "api", "labels": {"team": "payments"}}
        }
    }
}
```

Run:

```sh
akua test                       # runs all Rego + KCL tests
akua test --coverage            # includes per-rule coverage
akua test --watch               # TDD mode
```

Every test runs via the embedded OPA (see [embedded-engines.md](embedded-engines.md)). Output matches `opa test` structure; coverage format is compatible with standard OPA coverage tooling.

### Integration tests (render + policy)

The `akua.render` custom builtin lets policy tests exercise the full pipeline:

```rego
test_production_package_passes_tier {
    rendered := akua.render({
        "package": "oci://pkg.example.com/webapp:3.2",
        "inputs":  {"hostname": "example.com", "replicas": 3}
    })
    count(deny) == 0 with input as rendered
}

test_dev_inputs_still_fail_production_tier {
    rendered := akua.render({
        "package": "oci://pkg.example.com/webapp:3.2",
        "inputs":  {"hostname": "example.com", "replicas": 0}  # zero replicas
    })
    deny["production deployments need at least 2 replicas"] with input as rendered
}
```

No separate test framework. No mocking. The package gets rendered, the policy runs against it, assertions fire. End-to-end correctness verified in one place.

### Linting

```sh
akua lint
```

Runs:

- **Regal** (embedded) on `.rego` files — style rules, performance anti-patterns (top-level iteration, unused imports)
- **KCL lint** on `.k` files — style rules, missing docstrings, unreachable code
- **Cross-engine** checks unique to akua — e.g., a Package references a Policy that doesn't exist in the workspace

Output is structured per [cli.md](cli.md#akua-lint). Severity levels: warn, error. CI gates can require `--severity=error` be clean.

### Formatting

```sh
akua fmt                        # in-place
akua fmt --check                # fail CI if formatting needed
akua fmt --diff                 # preview changes without applying
```

Runs `opa fmt` (embedded) on `.rego` files and `kcl fmt` (embedded) on `.k` files. Both are idempotent.

### Tracing

When a policy denies and you need to understand why:

```sh
akua trace 'data.akua.policies.my_org_production.deny' --input=./deploy/api.yaml
```

Routes OPA's `--explain full` output. Shows rule-by-rule evaluation with booleans at each step. Invaluable for debugging complex compositions.

For agents: `akua trace --depth=full --json` emits structured event trees; agents parse to reason about why a rule did or didn't fire.

### Benchmarking

```sh
akua bench --policy=tier/production --input=./deploy/
```

Uses OPA partial evaluation to produce per-rule latency numbers. Critical for policies that run in admission webhooks (sub-millisecond budget) or at scale in CI.

### Coverage reports

```sh
akua cov --min=80               # fail if policy coverage is below 80%
akua cov --format=lcov          # produce lcov for code-review tooling
```

Embedded OPA's coverage report, rolled up across Rego files + imported tier bundles. Useful as a CI gate — policies that silently add rules without tests get caught.

### REPL

```sh
akua repl
> :mode rego
rego> data.akua.policies.production.deny with input as { resource: { kind: "Deployment" } }
[...]
rego> :trace
(switches to trace mode; next query shows full evaluation)
```

---

## 12. Relationship to other docs

- **[package-format.md](package-format.md)** — how `check:` blocks in KCL complement Rego
- **[lockfile-format.md](lockfile-format.md)** — how Rego imports are pinned
- **[cli.md — `akua policy` / `akua test` / `akua trace` / `akua bench`](cli.md)** — the verbs that operate on policy
- **[embedded-engines.md](embedded-engines.md)** — OPA, Regal, Kyverno-to-Rego converter, CEL all embedded via wasmtime
- **[skills/apply-policy-tier](../skills/apply-policy-tier/SKILL.md)** — agent workflow for subscribing to a tier
- **[skills/test-and-lint](../skills/test-and-lint/SKILL.md)** — agent workflow for setting up tests + lint gates
