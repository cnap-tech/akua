# Example 05 — tests and golden fixtures

Shows where tests live and what each kind looks like:

- **`test_*.k`** — KCL unit tests. Exercise the Package's `Input` schema, defaults, and `check:` blocks.
- **`*_test.rego`** — Rego policy tests. Feed fixtures to the policy package and assert verdicts.
- **`testdata/golden/<fixture>/`** — golden render output. `akua test --golden` renders the Package against each input under `testdata/inputs/` and diffs the result against the expected bytes. A drift is a test failure.

All three kinds run under one verb:

```sh
akua test                   # runs *_test.rego, test_*.k, golden fixtures
akua test --golden          # golden-only
akua test --update-golden   # overwrite golden with current render (use carefully)
```

## Layout

```
05-tests-and-golden/
├── akua.mod
├── akua.sum
├── package.k                     the Package under test
├── test_package.k                KCL unit tests
├── policies/
│   ├── workspace.rego            a local policy
│   └── workspace_test.rego       Rego policy tests
├── testdata/
│   ├── inputs/
│   │   ├── minimal.yaml          minimal valid input set
│   │   └── full.yaml             every optional field exercised
│   └── golden/
│       ├── minimal/              expected render output for minimal.yaml
│       └── full/                 expected render output for full.yaml
└── README.md
```

## The three kinds, compared

| kind | file pattern | what it catches | scope |
|---|---|---|---|
| KCL unit test | `test_*.k` | schema defaults, `check:` blocks, pure functions | one Package |
| Rego policy test | `*_test.rego` | allow / deny / needs-approval on sample inputs | one policy or policy bundle |
| Golden test | `testdata/golden/<name>/` | drift in rendered YAML bytes | end-to-end for a Package |

Different kinds catch different bugs. Use all three — they compound.

## KCL unit tests (`test_package.k`)

Top-level `assert` statements. Failures surface with line + field context.

```python
import .package as pkg

_sample = pkg.Input { appName = "checkout", hostname = "x.example.com" }

assert _sample.replicas == 3, "replicas default should be 3"
assert _sample.team == "platform", "team default should be 'platform'"
```

See `test_package.k` in this directory for the full set.

## Rego policy tests (`policies/workspace_test.rego`)

Standard OPA test shape — rules named `test_*` are discovered.

```rego
package akua.policies.workspace_test
import data.akua.policies.workspace

test_allows_conforming {
    count(workspace.deny) == 0 with input as {
        "resource": { ... },
    }
}

test_denies_missing_label {
    some msg in workspace.deny with input as { ... }
    contains(msg, "must have a team label")
}
```

## Golden tests

For each fixture under `testdata/inputs/`, `akua test --golden`:

1. Runs `akua render --inputs=testdata/inputs/<name>.yaml`.
2. Writes a temporary output.
3. Diffs against `testdata/golden/<name>/` byte-for-byte.
4. Fails the test on any drift, printing the diff.

Updates go through the human:

```sh
akua render --inputs=testdata/inputs/minimal.yaml --out=/tmp/render
diff -r /tmp/render testdata/golden/minimal/    # eyeball the drift
akua test --update-golden                        # commit the new expectation
```

Golden tests are the cheapest way to catch "I accidentally changed the output shape in a refactor" regressions. They're also the cheapest way to generate false positives when engine versions bump — the diff is the signal, not the test's opinion.

## When to add what

- Writing a Package for the first time → `test_*.k` for schema + defaults.
- Landing non-trivial rendering logic → add a golden fixture.
- Authoring a policy → always write `*_test.rego` alongside.
- Bumping an engine version (e.g. Helm v4 → v4.1) → run `akua test --golden` first; expect some drift, review it, update if sound.

## See also

- [cli.md `test`](../../docs/cli.md) — verb reference, flags, exit codes
- [policy-format.md §7 Testing](../../docs/policy-format.md) — Rego test conventions
- [package-format.md §11 Testing](../../docs/package-format.md) — KCL test conventions
- [skills/test-and-lint/](../../skills/test-and-lint/SKILL.md) — agent workflow
