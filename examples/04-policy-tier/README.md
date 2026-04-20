# Example 04 — policy tier

A workspace-level policy gate. Shows the full policy stack in one place:

- Authoring a Rego policy on top of an imported signed tier
- Bringing in Kyverno rules via compile-resolved import (not runtime string lookup)
- Composing tiers into a `PolicySet` as typed KCL
- Running `akua policy check` against passing + failing fixtures
- A `*_test.rego` file showing the test shape

This is the smallest example that exercises every part of the policy architecture described in [policy-format.md](../../docs/policy-format.md).

## Layout

```
04-policy-tier/
├── akua.mod                       declared policy deps (OCI-pinned)
├── akua.sum                       digest + signature ledger (machine-maintained)
├── policies/
│   ├── production.rego            local tier extending imports
│   └── production_test.rego       unit tests for the local rules
├── policy-set.k                   PolicySet KRM (typed KCL; see krm-vocabulary.md)
├── fixtures/
│   ├── good.yaml                  App manifest that passes the tier
│   └── bad.yaml                   App manifest missing the required team label
└── README.md
```

## The policy stack, layer by layer

### 1. Declared deps — `akua.mod`

```toml
[dependencies]
tier-prod = { oci = "oci://policies.akua.dev/tier/production", version = "1.2.0" }
kyv-sec   = { oci = "oci://policies.akua.dev/kyverno/security", version = "2.0.0" }
```

Both deps are signed OCI artifacts. The first is akua's reference `tier/production` Rego bundle; the second is a Kyverno bundle that akua converts to Rego at `akua add` time (stored under `.akua/policies/vendor/`). The `akua.sum` ledger records the resolved digest and cosign signature for each.

No runtime lookups. Every import resolves at build time.

### 2. Local rules — `policies/production.rego`

Inherits rules from the two imports and adds a cross-resource aggregation rule that's specific to this workspace.

### 3. PolicySet — `policy-set.k`

Composes the local Rego policies into a workspace-scoped PolicySet. This is **KCL, not YAML** — PolicySet is a control-plane KRM kind (see [krm-vocabulary.md §control-plane-kinds](../../docs/krm-vocabulary.md)). YAML is only a derived view via `akua export`.

## Running it

```sh
# 1. Resolve deps + write akua.sum
akua add                     # no arg → reads akua.mod, fetches, signs-check, writes akua.sum

# 2. Evaluate the tier against a passing fixture → verdict: allow
akua policy check --tier=./policies --input=fixtures/good.yaml

# 3. Same against a failing fixture → verdict: deny, with line-precise reason
akua policy check --tier=./policies --input=fixtures/bad.yaml

# 4. Run the test file
akua test policies/

# 5. Inspect the PolicySet as YAML (derived view)
akua export policy-set.k --format=yaml
```

Exit codes from `akua policy check`:

- `0` — allow
- `3` — policy deny (the verdict was `deny`; rule violations printed to stdout as JSON when `--json` is passed)
- `5` — needs-approval (the verdict was `needs-approval`; human review required before the change proceeds)

## The shape of a deny response

```sh
$ akua policy check --tier=./policies --input=fixtures/bad.yaml --json
{
  "verdict": "deny",
  "violations": [
    {
      "rule": "akua.policies.my_org_production.deny",
      "message": "production Deployments must have a team label",
      "resource": { "kind": "Deployment", "name": "checkout" },
      "path": "metadata.labels.team",
      "source": "policies/production.rego:18"
    }
  ]
}
```

Line + field precision. Agent-parseable. No stderr surprises.

## See also

- [policy-format.md](../../docs/policy-format.md) — canonical spec
- [lockfile-format.md](../../docs/lockfile-format.md) — how `akua.mod` + `akua.sum` work
- [krm-vocabulary.md](../../docs/krm-vocabulary.md) — why `PolicySet` is typed KCL, not YAML
- [cli.md `policy check`](../../docs/cli.md) — verb reference
