# akua examples

Working examples of akua Packages, Apps, Environments, and Policies. Each directory is a standalone example you can copy, edit, and render locally.

> **Status, as of today.** Example 00 renders end-to-end through
> the shipping binary (with `--features engine-helm-shell` + `helm` on
> PATH). Examples 01–07 are *aspirational* — they illustrate the full
> target authoring shape but exercise surfaces that aren't wired
> yet: the `charts.*` KCL stdlib, kro RGD support, kustomize support,
> and the policy engine. The `akua.helm` / `akua.pkg` wrappers
> *do* ship — examples 00 + 08 use them.

---

## The examples

| # | directory | what it shows | renders today? |
|---|---|---|---|
| 00 | [00-helm-hello/](00-helm-hello/) | simplest Package exercising `helm.template` against a bundled chart | ✅ via embedded `helm-engine-wasm` |
| 01 | [01-hello-webapp/](01-hello-webapp/) | simplest Package: one schema input, one Helm chart, docstrings + `@ui` decorators | ⏳ needs typed `charts.*` deps (roadmap Phase 2) + `helm-engine-wasm` |
| 02 | [02-webapp-postgres/](02-webapp-postgres/) | cross-source wiring — a webapp consuming a CNPG-managed Postgres secret via convention; `test_package.k` | ⏳ Phases 1 + 2 |
| 03 | [03-multi-env-app/](03-multi-env-app/) | Package + App + Environment as typed KCL — the full workspace authoring shape | ⏳ Phases 1 + 2 |
| 04 | [04-policy-tier/](04-policy-tier/) | Rego tier + Kyverno compile-resolved import, with passing + failing fixtures; shows that policy composition is just Rego file layout — no akua-owned PolicySet kind | ⏳ needs the policy engine |
| 05 | [05-tests-and-golden/](05-tests-and-golden/) | `test_*.k` + `*_test.rego` + golden-fixture render snapshots — the three kinds of tests side by side | ⏳ Phases 1 + 2 + test runner |
| 06 | [06-multi-engine/](06-multi-engine/) | Helm + Kustomize + kro RGD + inline KCL resources in one Package, flattened into one raw-manifest render | ⏳ Phases 1 + 3 + `kro.rgd` transformation |
| 07 | [07-package-reuse/](07-package-reuse/) | one akua Package composing another via `pkg.render()` — nested `Input` schemas, OCI-pinned base, attestation-chain provenance | ⏳ needs OCI fetch (path-based composition works — see 08) |
| 08 | [08-pkg-compose/](08-pkg-compose/) | pure-KCL Package-of-Packages composition — outer calls `pkg.render("./shared", …)` twice, renders two ConfigMaps | ✅ |
| 09 | [09-kustomize-hello/](09-kustomize-hello/) | smallest `kustomize.build` example — overlay adds a namePrefix + labels to a base ConfigMap | ⏳ waiting on `kustomize-engine-wasm` (roadmap Phase 3) |

What **does** run today:

- Any pure-KCL Package (no engine imports). The `akua init` scaffold is a minimal working example.
- `examples/00-helm-hello/` — embedded WASM Helm engine. Requires `task build:helm-engine-wasm` once.
- `examples/08-pkg-compose/` — pure-KCL Package-of-Packages composition via `pkg.render`.

Example 09 used to shell out to `kustomize`. That path was removed in Phase 0 — akua doesn't shell out from the render path, ever. The Package stays as the target shape for the embedded kustomize WASM engine (Phase 3 of [`docs/roadmap.md`](../docs/roadmap.md)).

---

## How to use

Install akua:

```sh
cargo install --git https://github.com/cnap-tech/akua akua-cli
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
- **05 → 06:** demonstrates multi-engine composition — Helm + Kustomize + kro RGD + inline KCL, all flattened into one `resources` list, one raw-manifest render.
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
