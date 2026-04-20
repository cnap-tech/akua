---
name: test-and-lint
description: Set up tests, linting, formatting, coverage, and tracing for an akua workspace — for Packages (KCL) and Policies (Rego) together. Use when bootstrapping CI for a new platform repo, adding test coverage to an untested package, debugging a policy that denies unexpectedly, configuring a format/lint pre-commit hook, or when asked "add tests" / "set up CI gates" / "why is this policy failing?"
license: Apache-2.0
---

# Test, lint, format, trace

akua embeds the full testing and debugging surface from its host engines (KCL, OPA/Rego, Regal, Kyverno converter, CEL) into one binary. No `opa` or `kcl` or `regal` on `$PATH` required. See [docs/embedded-engines.md](../../docs/embedded-engines.md) for details.

## When to use

- Bootstrapping CI for a new akua workspace
- Adding test coverage to an existing Package or Policy that shipped without tests
- Configuring pre-commit hooks for formatting and linting
- Debugging why a specific policy denies (or doesn't)
- Setting up a benchmark gate for high-throughput policies (admission, CI at scale)

## Steps

### 1. Add test files

Test naming conventions (discovered automatically by `akua test`):

- **KCL**: `test_*.k` or `*_test.k` anywhere under a Package
- **Rego**: `*_test.rego` next to the policy files
- **Golden**: `expected.golden.yaml` next to inputs in `tests/<case>/`

Minimum: one happy-path test and one negative-path test per rule / schema.

Example Rego test:

```rego
# policies/production_test.rego
package akua.policies.my_production

import future.keywords

test_deny_missing_team_label {
    deny[_] with input as {
        "resource": {"kind": "Deployment", "metadata": {"name": "api", "labels": {}}}
    }
}

test_allow_with_team_label {
    count(deny) == 0 with input as {
        "resource": {"kind": "Deployment", "metadata": {"name": "api", "labels": {"team": "payments"}}}
    }
}
```

Example KCL test:

```python
# test_schema.k
import package as pkg

_default_sample = pkg.Input {
    appName:  "test"
    hostname: "test.example.com"
}

assert _default_sample.replicas == 3, "default replicas should be 3"
```

### 2. Run tests locally

```sh
akua test                       # runs everything
akua test --coverage            # with coverage report
akua test --watch               # TDD; re-runs on file change
akua test --filter=<regex>      # only matching tests
```

Confirm all tests pass before committing. Coverage below 80% on policies is a smell.

### 3. Format + lint

```sh
akua fmt                        # in-place format
akua lint                       # style + correctness
akua check                      # syntax/type-only, fastest gate
```

Expected output on a clean workspace: zero issues. Any lint warnings include the `rule` name and usually a `fix` suggestion; apply with `akua lint --fix` where auto-fixable.

### 4. Wire pre-commit hooks

`.pre-commit-config.yaml`:

```yaml
repos:
  - repo: local
    hooks:
      - id: akua-fmt
        name: akua fmt
        entry: akua fmt --check
        language: system
        pass_filenames: false
      - id: akua-lint
        name: akua lint
        entry: akua lint --severity=error
        language: system
        pass_filenames: false
      - id: akua-check
        name: akua check
        entry: akua check
        language: system
        pass_filenames: false
```

Fast feedback at commit time. `akua check` is the cheapest syntax/type pass; runs in under 100 ms for typical workspaces.

### 5. Wire CI gates

`.github/workflows/akua-test.yml`:

```yaml
name: akua test
on: [pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install akua
        run: curl -fsSL https://akua.dev/install | sh
      - name: Check + lint
        run: |
          akua check
          akua lint --severity=error
          akua fmt --check
      - name: Test with coverage
        run: akua test --coverage --min=80
      - name: Verify lockfile
        run: akua verify
      - name: Integration test — render + policy
        run: |
          akua render --env production --out ./rendered
          akua policy check --tier tier/production --target ./rendered
```

The integration step catches cases a unit test misses — the full render + policy pipeline in production mode.

### 6. Debug a policy that denies unexpectedly

When a policy denies and it's not obvious why:

```sh
akua trace 'data.akua.policies.my_production.deny' --input=./deploy/api.yaml --depth=full
```

Shows every rule evaluated, every boolean at each step, which rule fired and why. Read top-down.

Common patterns:

- "Expected rule to fire but didn't" — check that every condition in the rule body is `TRUE` in the trace; any `FALSE` short-circuits the rule.
- "Unexpected denial" — find the rule that fired (marked `ALLOW` / result-producing); work through its conditions.
- "Different verdict in CI than local" — check `akua version --json` on both; embedded engine version mismatch is a common cause.

### 7. Benchmark if latency matters

For admission webhooks or high-throughput evaluators:

```sh
akua bench --policy=tier/production --input=./rendered --iterations=1000
```

Target: p99 under 10 ms for tier/production. If above, profile the failing rules — usually one poorly-written aggregation is responsible.

### 8. Enforce minimums in CI

Set minimum thresholds for policy quality:

```yaml
- name: Coverage gate
  run: akua cov --min=80
- name: Benchmark gate
  run: akua bench --policy=tier/production --p99-max-ms=10
- name: Ratio of tests-to-rules
  run: akua lint --severity=error  # fails if any rule has no test coverage
```

## Agent-specific guidance

When an agent is asked to fix a failing policy or add tests:

1. **Always `akua test --json` first** to see the failure shape in structured form.
2. **Use `akua trace --json`** for any unexplained denial before proposing a fix; don't guess.
3. **Add a regression test** before fixing — the test should fail in the current state and pass after the fix. This is verifiable by running `akua test` before and after.
4. **Prefer `akua lint --fix`** for auto-fixable style issues rather than editing manually.
5. **Update `akua.sum`** via `akua verify --update` if signatures or digests drifted after a dep bump.

## Failure modes

- **`E_TEST_FILE_NOT_FOUND`** — pattern `*_test.rego` or `test_*.k` matched nothing. Probably misnamed files.
- **`E_COVERAGE_BELOW_MIN`** (exit 1) — CI gate failed. Add tests for uncovered rules; re-run with `--coverage` to see which.
- **`E_FMT_NEEDED`** (exit 1 under `--check`) — run `akua fmt` to auto-apply.
- **`E_LINT_ERROR`** — severity:error issues. Not auto-fixable; author must address.
- **`E_BENCH_REGRESSION`** — p99 exceeded threshold. Policy added a slow rule; profile and optimize.

## Reference

- [cli.md — akua test / fmt / lint / check / bench / trace / cov / repl / eval](../../docs/cli.md)
- [package-format.md §11 — Testing Packages](../../docs/package-format.md)
- [policy-format.md §11 — Testing, linting, tracing](../../docs/policy-format.md)
- [embedded-engines.md](../../docs/embedded-engines.md) — which engines drive which verbs
- [diff-gate](../diff-gate/SKILL.md) — complementary CI gate for upgrade compatibility
