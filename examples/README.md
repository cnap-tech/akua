# akua examples

Working examples of akua Packages, Apps, Environments, and Policies. Each directory is a standalone example you can copy, edit, and render locally.

> **Status, as of today.** Example 00 renders end-to-end through
> the shipping binary (with `--features engine-helm-shell` + `helm` on
> PATH). Examples 01–07 are *aspirational* — they illustrate the full
> target authoring shape but exercise surfaces that aren't wired
> yet: the `akua.helm` / `charts.*` KCL stdlib (users write
> `import kcl_plugin.helm` directly today), kro RGD support,
> kustomize support, the policy engine, and `pkg.render()`.

---

## The examples

| # | directory | what it shows | renders today? |
|---|---|---|---|
| 00 | [00-helm-hello/](00-helm-hello/) | simplest working example: `kcl_plugin.helm.template` against a bundled chart, ConfigMap out | ✅ with `engine-helm-shell` |
| 01 | [01-hello-webapp/](01-hello-webapp/) | simplest Package: one schema input, one Helm chart, docstrings + `@ui` decorators | ❌ uses `akua.helm` stdlib (not yet shipped) |
| 02 | [02-webapp-postgres/](02-webapp-postgres/) | cross-source wiring — a webapp consuming a CNPG-managed Postgres secret via convention; `test_package.k` | ❌ needs `akua.helm` stdlib |
| 03 | [03-multi-env-app/](03-multi-env-app/) | Package + App + Environment as typed KCL — the full workspace authoring shape | ❌ needs `akua.helm` stdlib |
| 04 | [04-policy-tier/](04-policy-tier/) | Rego tier + Kyverno compile-resolved import, with passing + failing fixtures; shows that policy composition is just Rego file layout — no akua-owned PolicySet kind | ❌ needs the policy engine |
| 05 | [05-tests-and-golden/](05-tests-and-golden/) | `test_*.k` + `*_test.rego` + golden-fixture render snapshots — the three kinds of tests side by side | ❌ needs `akua.helm` stdlib + test runner |
| 06 | [06-multi-engine/](06-multi-engine/) | Helm + Kustomize + kro RGD + inline KCL resources in one Package, with per-source output routing | ❌ needs kustomize + kro engines |
| 07 | [07-package-reuse/](07-package-reuse/) | one akua Package composing another via `pkg.render()` — nested `Input` schemas, OCI-pinned base, attestation-chain provenance | ❌ needs OCI fetch (path-based composition works — see 08) |
| 08 | [08-pkg-compose/](08-pkg-compose/) | pure-KCL Package-of-Packages composition — outer calls `pkg.render("./shared", …)` twice, renders two ConfigMaps | ✅ |

What **does** run today:

- Any pure-KCL Package (no engine imports). The `akua init` scaffold is a minimal working example.
- `examples/00-helm-hello/` — exercises `helm.template` when built with the `engine-helm-shell` feature and a `helm` binary on PATH.
- `examples/08-pkg-compose/` — pure-KCL Package-of-Packages composition via `pkg.render`.

---

## How to use

Install akua with the helm engine opted in:

```sh
cargo install --git https://github.com/cnap-tech/akua akua-cli \
    --features akua-core/engine-helm-shell
```

Render the pure-KCL scaffold:

```sh
akua init my-pkg && cd my-pkg
akua render --out ./deploy
```

Render the helm-backed example:

```sh
cd examples/00-helm-hello
akua render --out ./deploy
ls deploy/
```

Once the `akua.helm` / `charts.*` KCL stdlib ships (Phase B follow-up):

```sh
cd examples/02-webapp-postgres
akua render --inputs inputs.yaml --out ./rendered
```

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
- [lockfile-format.md](../docs/lockfile-format.md) — `akua.toml` + `akua.lock`
- [sdk.md](../docs/sdk.md) — TypeScript SDK
- [architecture.md](../docs/architecture.md) — why the pipeline is shaped this way
