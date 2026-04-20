# akua examples

Working examples of akua Packages and Apps. Each directory is a standalone example you can copy, edit, and render locally.

Every example renders without a cluster. Run `akua render` in any subdirectory and get committable raw YAML.

---

## The examples

| # | directory | what it shows |
|---|---|---|
| 01 | [hello-webapp/](01-hello-webapp/) | simplest possible Package: one schema input, one Helm chart |
| 02 | [webapp-postgres/](02-webapp-postgres/) | cross-source wiring — a webapp consuming a CNPG-managed Postgres secret via convention |
| 03 | [multi-env-app/](03-multi-env-app/) | Package + App + Environment KRMs together — the full authoring shape |

---

## How to use

Install akua:

```sh
curl -fsSL https://akua.dev/install | sh
```

Render any example:

```sh
cd docs/examples/02-webapp-postgres
akua render --inputs inputs.yaml --out ./rendered
```

Inspect the resulting YAML. Diff between examples to learn the progressive additions.

---

## Reading order

If you're new to akua, read them in order. Each example adds exactly one concept over the prior one:

- **01 → 02:** adds a second source, cross-source value wiring, and output routing.
- **02 → 03:** separates the Package (reusable) from the App (per-install) — the shape most production workspaces use.

Beyond 03, realistic packages combine these patterns at scale. See the [use cases](../use-cases.md) for archetypes (solo dev, small SaaS, platform team, ISV).

---

## Related reference

- [cli.md](../cli.md) — the `akua` CLI surface
- [cli-contract.md](../cli-contract.md) — the universal verb invariants
- [sdk.md](../sdk.md) — TypeScript SDK
- [architecture.md](../architecture.md) — why the pipeline is shaped this way
