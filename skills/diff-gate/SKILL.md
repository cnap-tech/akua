---
name: diff-gate
description: Set up a CI gate that runs akua diff on package upgrades and blocks merges that break schema compatibility or violate policy. Use when configuring CI for a platform repo, preventing breaking Helm-chart upgrades, gating Renovate or Dependabot PRs, or enforcing structural-compatibility checks before deployment.
license: Apache-2.0
---

# CI gate using `akua diff`

Dependency bumps (Renovate, Dependabot, or a human edit) can silently break production. `akua diff` returns a structural diff between two package versions and exits non-zero if schema fields change incompatibly. Wire it into CI to block bad merges.

## When to use

- Any repo using akua packages where upgrades happen via PRs
- Post-installation of Renovate / Dependabot against a platform repo
- As a policy requirement for production-tier deploys
- Before adopting a new version of a third-party package

## What `akua diff` compares

| category | blocks merge? | example |
|---|---|---|
| New required schema field | yes | `adminEmail` added with no default |
| Schema field type changed | yes | `replicas: string` → `int` |
| Schema field removed | yes | downstream App still references it |
| Source chart major version bump | warn | `cnpg: 0.x → 1.x` |
| Policy compatibility change | yes | new version triggers policy denial |
| Default value shift | warn | `replicas: 3 → 5` |
| Rendered-manifest change only | info | cosmetic / safe |

Non-zero exit on any "yes"; warnings surface as PR comments without blocking.

## Steps

### 1. Add the workflow

Create `.github/workflows/akua-diff.yml`:

```yaml
name: akua diff

on:
  pull_request:
    paths:
      - 'apps/**'
      - 'environments/**'
      - 'policies/**'

jobs:
  diff:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with: { fetch-depth: 0 }

      - name: Install akua
        run: curl -fsSL https://akua.dev/install | sh

      - name: Diff against base branch
        run: |
          for app in apps/*/; do
            base_ref=$(git show origin/${{ github.base_ref }}:${app}app.yaml | yq '.spec.package')
            head_ref=$(yq '.spec.package' ${app}app.yaml)

            if [ "$base_ref" != "$head_ref" ]; then
              echo "::group::Diff for ${app} ($base_ref → $head_ref)"
              akua diff "$base_ref" "$head_ref" --json > diff.json || EXIT=$?
              akua diff "$base_ref" "$head_ref"    # human-readable for logs
              echo "::endgroup::"
            fi
          done

          exit ${EXIT:-0}

      - name: Post diff as PR comment
        if: always()
        uses: actions/github-script@v7
        with:
          script: |
            const fs = require('fs');
            if (!fs.existsSync('diff.json')) return;
            const diff = JSON.parse(fs.readFileSync('diff.json'));
            const body = formatDiff(diff);  // see scripts/format-diff.js
            github.rest.issues.createComment({
              issue_number: context.issue.number,
              owner: context.repo.owner,
              repo: context.repo.repo,
              body
            });
```

### 2. Make it required

In GitHub repo settings → Branches → the protected branch → "Require status checks to pass" — add the `diff` check as required. Now no PR merges without a clean diff or explicit override.

### 3. Test the gate

Make a test PR that bumps a package to a version with a breaking schema change. The workflow should fail. Revert the PR; the workflow should pass. Confirm both paths work before trusting the gate.

## Policy-aware gating

Enable policy-tier checking in the same workflow:

```yaml
      - name: Policy check
        run: |
          akua render --env production --out ./rendered
          akua policy check --tier tier/production --target ./rendered --json > verdict.json

          verdict=$(jq -r '.verdict' verdict.json)
          if [ "$verdict" = "deny" ]; then
            echo "::error::Policy denied. See verdict.json for failing rules."
            exit 3
          fi
          if [ "$verdict" = "needs-approval" ]; then
            echo "::warning::Needs approval. Contact approvers listed in verdict.json."
          fi
```

Exit code 3 (policy deny) is a distinct failure mode from exit 1 (schema breaking). CI can branch on exit code:

- 0 → merge
- 1 → schema-breaking change; reject
- 3 → policy deny; reject
- 5 → needs approval; post for human review, do not auto-merge

## Renovate integration

Renovate can be configured to run `akua diff` as a post-upgrade task:

```json
// renovate.json
{
  "postUpgradeTasks": {
    "commands": [
      "akua diff {{baseBranch}}@{{package}} HEAD@{{package}} --json > diff.json"
    ],
    "fileFilters": ["diff.json"]
  }
}
```

The diff attaches to the Renovate PR body automatically.

## Failure modes

- **`E_DIFF_FETCH_FAILED`** — cannot fetch one of the two package versions. Usually registry auth; run `akua login` locally and ensure CI has the same credentials.
- **Workflow passes but upgrade still breaks at deploy** — means the breaking change wasn't in the schema (it was in the rendered YAML's semantics). Consider tightening your policy tier to cover rendered-manifest-level checks.
- **False positives on cosmetic renders** — some Helm charts produce non-deterministic rendering (timestamps, random suffixes). File a bug against the upstream chart or pin to a known-deterministic version.

## Reference

- [cli.md — akua diff](../../docs/cli.md#akua-diff)
- [cli.md — akua policy](../../docs/cli.md#akua-policy)
- [cli-contract.md — typed exit codes](../../docs/cli-contract.md#2-exit-codes)
