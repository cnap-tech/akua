# Example 04 — policy tier

A workspace policy gate. Shows the policy stack end-to-end:

- Authoring a Rego policy on top of an imported signed tier
- Bringing in Kyverno rules via compile-resolved import (not runtime string lookup)
- Running `akua policy check` against passing + failing fixtures
- A `*_test.rego` file showing the test shape

This is the smallest example that exercises every part of the policy architecture described in [policy-format.md](../../docs/policy-format.md).

akua does **not** ship a `PolicySet` kind. Composition happens as plain Rego file layout: local `.rego` files import tiers as compile-resolved `data.*` via `akua.toml`. The workspace's policy layout is the workspace's concern.

## Layout

```
04-policy-tier/
├── akua.toml                       declared policy deps (OCI-pinned)
├── akua.lock                       digest + signature ledger (machine-maintained)
├── policies/
│   ├── production.rego            local tier extending imported tiers
│   └── production_test.rego       unit tests for the local rules
├── fixtures/
│   ├── good.yaml                  resource + environment input that passes
│   └── bad.yaml                   resource missing the required team label
└── README.md
```

## The policy stack, layer by layer

### 1. Declared deps — `akua.toml`

```toml
[dependencies]
tier-prod = { oci = "oci://policies.akua.dev/tier/production", version = "1.2.0" }
kyv-sec   = { oci = "oci://policies.akua.dev/kyverno/security", version = "2.0.0" }
```

Both deps are signed OCI artifacts. The first is akua's reference `tier/production` Rego bundle; the second is a Kyverno bundle that akua converts to Rego at `akua add` time (stored under `.akua/policies/vendor/`). The `akua.lock` ledger records the resolved digest and cosign signature for each.

No runtime lookups. Every import resolves at build time.

### 2. Local rules — `policies/production.rego`

Inherits rules from the two imports and adds a cross-resource aggregation rule specific to this workspace (per-env CPU budget, team-label requirement).

### 3. Composition is just Rego

There is no `PolicySet` resource to declare. `akua policy check --tier=./policies` evaluates the Rego package under `./policies/` with the imports resolved from `akua.toml`. If you want to compose multiple local policy packages, lay them out under `./policies/<name>/` — Rego's own `import` + rule-merging is the composition mechanism.

## Running it

```sh
# 1. Resolve deps + write akua.lock
akua add

# 2. Evaluate the tier against a passing fixture → verdict: allow
akua policy check --tier=./policies --input=fixtures/good.yaml

# 3. Same against a failing fixture → verdict: deny, with line-precise reason
akua policy check --tier=./policies --input=fixtures/bad.yaml

# 4. Run the test file
akua test policies/
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

- [policy-format.md](../../docs/policy-format.md) — canonical Rego spec
- [lockfile-format.md](../../docs/lockfile-format.md) — how `akua.toml` + `akua.lock` work
- [cli.md `policy check`](../../docs/cli.md) — verb reference
