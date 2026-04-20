---
name: apply-policy-tier
description: Subscribe to a curated policy tier (dev, startup, production, soc2, hipaa, fedramp-moderate) and apply it across a workspace. Use when preparing for a compliance audit, hardening a production environment, standardizing guardrails across teams, onboarding a new environment, or when a user asks "make this SOC2-ready" / "apply production policy".
license: Apache-2.0
---

# Apply a policy tier to a workspace

Policy tiers are curated signed bundles of rules that govern what deploys can do. akua ships built-in tiers covering dev → compliance regimes. Applying a tier is three steps: install, assign, remediate.

## When to use

- Onboarding a new `Environment` (set `policy: tier/production`)
- Tightening rules before a compliance audit (SOC2, HIPAA, FedRAMP)
- Standardizing policy across multi-team workspaces
- Moving from `tier/dev` to `tier/production` as the app matures

## Tier overview

| tier | baseline | adds |
|---|---|---|
| `tier/dev` | permissive defaults for local / exploratory use | — |
| `tier/startup` | resource limits, non-privileged, basic probes | — |
| `tier/production` | + budget caps, required approvals, soak gates, anti-affinity | — |
| `tier/soc2` | production + | audit retention, access logging, change tracking |
| `tier/hipaa` | production + | encryption-at-rest mandates, network segmentation |
| `tier/fedramp-moderate` | production + | FIPS crypto, stricter ingress, logging regime |

Each is a signed OCI artifact. You can fork and publish custom tiers using the same format.

## Steps

### 1. List available tiers

```sh
akua policy tiers --json
```

Shows installed + available-for-install tiers with their current subscribed version.

### 2. Install a tier

```sh
akua policy install tier/production
```

Pulls the signed policy bundle, verifies the signature, caches it locally. Does not yet apply it anywhere.

For custom tiers:

```sh
akua policy install tier/my-org-hardening --from=oci://pkg.my-org.com/policies/hardening:v2
```

### 3. Dry-run against current workspace

Before assigning, see what would fail:

```sh
akua policy check --tier tier/production --target ./deploy/production --json
```

Output:

```json
{
  "tier": "tier/production",
  "verdict": "deny",
  "failing": [
    {
      "rule": "budget_cap",
      "resource": "Deployment/api",
      "reason": "replicas * resources.requests.cpu exceeds team budget",
      "suggested_fix": "reduce replicas to 3 or increase budget to $500/mo"
    },
    {
      "rule": "anti_affinity_required",
      "resource": "Deployment/worker",
      "reason": "replicas > 1 without anti-affinity rule",
      "suggested_fix": "add spec.affinity.podAntiAffinity to the Package"
    }
  ]
}
```

Agents should fix these before assigning the tier.

### 4. Remediate

For each failing rule:

- **Schema-level** (missing input, wrong default) → update `package.k` schema
- **Rendered-manifest-level** (missing probe, no resource limit) → add to the source engine call or the `postRenderer` lambda
- **Environment-level** (budget exceeded) → update `environments/<env>.yaml` budget or reduce footprint

Re-run `akua policy check` until verdict is `allow`.

### 5. Assign the tier to an environment

Update `environments/<env>.yaml`:

```yaml
apiVersion: akua.dev/v1alpha1
kind: Environment
metadata:
  name: production
spec:
  policy: tier/production   # was: tier/startup
  cluster: prod-eu
  region:  eu-central-1
  budget: { monthly: "$1500" }
```

Commit + PR. CI runs `akua policy check` on the PR against the rendered output; if verdict is anything other than `allow`, the PR is blocked (see [diff-gate](../diff-gate/SKILL.md) skill).

### 6. Monitor ongoing policy checks

Once the tier is assigned, every `akua deploy` and `akua dev` session checks against it:

```sh
akua deploy --to=argo               # policy check runs automatically
akua dev --policy=tier/production   # live re-check in the dev loop
```

## Compliance tiers (SOC2, HIPAA, FedRAMP)

Compliance tiers layer on top of `tier/production`. They add audit / crypto / retention rules that the compliance regime requires.

To move from `tier/production` to `tier/soc2`:

```sh
akua policy install tier/soc2
akua policy diff tier/production tier/soc2 --json   # see what's added
akua policy check --tier tier/soc2 --target ./deploy/production --json
```

Remediate the delta, then assign `policy: tier/soc2` in the Environment. The audit spine automatically tightens retention; access logs tighten scope; change-tracking strengthens.

## Fork a tier for custom rules

```sh
akua policy fork tier/production --as tier/my-org-hardening
# edit ./policies/tier-my-org-hardening/
akua policy publish tier/my-org-hardening --to oci://pkg.my-org.com/policies/hardening
```

Forked tiers stay connected to the upstream — when upstream `tier/production` ships a policy update, `akua policy update tier/my-org-hardening` pulls those and you resolve any conflicts.

## Failure modes

- **`E_POLICY_VERIFY_FAILED`** — policy bundle signature does not verify. Do not use. Likely tampering or wrong cosign key.
- **`E_POLICY_DENY`** (exit 3) during deploy — the assigned tier rejects something that used to pass. Rollback: either revert the recent change, or temporarily downgrade the environment's tier (requires approval).
- **False positives on legit deploys** — if a rule is consistently wrong, open an issue against the tier. Don't disable the rule locally without a conversation.
- **Compliance tier too strict for dev** — use `tier/dev` locally, `tier/production` + `tier/soc2` for higher envs. The Environment resource carries the policy per-env.

## Reference

- [cli.md — akua policy](../../docs/cli.md#akua-policy)
- [diff-gate](../diff-gate/SKILL.md) — enforce policy in CI
- [rotate-secret](../rotate-secret/SKILL.md) — secret-specific policy rules
- Policy tier catalog: `docs/policies/` (forthcoming)
