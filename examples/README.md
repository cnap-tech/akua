# akua examples

Working examples of akua Packages, Apps, Environments, and Policies. Each directory is a standalone example you can copy, edit, and render locally.

Every example renders without a cluster. Run `akua render` (or `akua policy check` for 04) in any subdirectory and get committable output.

---

## The examples

| # | directory | what it shows |
|---|---|---|
| 01 | [01-hello-webapp/](01-hello-webapp/) | simplest Package: one schema input, one Helm chart, docstrings + `@ui` decorators |
| 02 | [02-webapp-postgres/](02-webapp-postgres/) | cross-source wiring — a webapp consuming a CNPG-managed Postgres secret via convention; `test_package.k` |
| 03 | [03-multi-env-app/](03-multi-env-app/) | Package + App + Environment as typed KCL — the full workspace authoring shape |
| 04 | [04-policy-tier/](04-policy-tier/) | Rego tier + Kyverno compile-resolved import, with passing + failing fixtures; shows that policy composition is just Rego file layout — no akua-owned PolicySet kind |
| 05 | [05-tests-and-golden/](05-tests-and-golden/) | `test_*.k` + `*_test.rego` + golden-fixture render snapshots — the three kinds of tests side by side |
| 06 | [06-multi-engine/](06-multi-engine/) | Helm + Kustomize + kro RGD + inline KCL resources in one Package, with per-source output routing |
| 07 | [07-package-reuse/](07-package-reuse/) | one akua Package composing another via `pkg.render()` — nested `Input` schemas, OCI-pinned base, attestation-chain provenance |

---

## How to use

Install akua:

```sh
curl -fsSL https://akua.dev/install | sh
```

Render any example:

```sh
cd examples/02-webapp-postgres
akua render --inputs inputs.yaml --out ./rendered
```

Check a policy:

```sh
cd examples/04-policy-tier
akua add                 # resolve deps → writes akua.sum
akua policy check --tier=./policies --input=fixtures/good.yaml
akua policy check --tier=./policies --input=fixtures/bad.yaml
akua test policies/
```

Inspect the resulting YAML. Diff between examples to learn the progressive additions.

---

## Reading order

Each example adds exactly one concept over the prior one:

- **01 → 02:** adds a second source, cross-source value wiring, and a unit-test file.
- **02 → 03:** separates the Package (reusable, OCI-published) from the App (per-install) — the shape most production workspaces use.
- **03 → 04:** adds the policy stack — Rego tier, compile-resolved Kyverno import, plain Rego composition (no akua-owned PolicySet kind), tests, and fixture-driven verdicts.
- **04 → 05:** formalizes testing — KCL unit tests, Rego policy tests, and golden render fixtures side by side.
- **05 → 06:** demonstrates multi-engine composition — Helm + Kustomize + kro RGD + inline KCL in one Package with named output routing.
- **06 → 07:** introduces package-of-packages composition — `pkg.render()` consumes a pinned base Package the same way `helm.template()` consumes a chart. The shape cross-package reuse takes.

Beyond 07, realistic workspaces combine these patterns at scale. See the [use cases](../docs/use-cases.md) for archetypes (solo dev, small SaaS, platform team, ISV).

---

## Related reference

- [cli.md](../docs/cli.md) — the `akua` CLI surface
- [cli-contract.md](../docs/cli-contract.md) — the universal verb invariants
- [package-format.md](../docs/package-format.md) — Package authoring spec
- [policy-format.md](../docs/policy-format.md) — Policy authoring spec
- [lockfile-format.md](../docs/lockfile-format.md) — `akua.mod` + `akua.sum`
- [sdk.md](../docs/sdk.md) — TypeScript SDK
- [architecture.md](../docs/architecture.md) — why the pipeline is shaped this way
