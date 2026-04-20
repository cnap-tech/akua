# Example 02 — webapp-postgres

Two Helm charts composed into one Package. A webapp consumes a Postgres connection URL from a Secret the CloudNativePG operator creates — entirely by convention, no runtime late-binding required, all deterministic at CI time. Adds a `postRenderer` lambda to label every rendered resource with the owning team. Demonstrates a `test_package.k` unit-test file.

## Layout

```
02-webapp-postgres/
├── akua.toml             declares charts.cnpg and charts.webapp
├── akua.lock             digest + signature ledger
├── package.k            the Package — two helm.template() calls + aggregation
├── test_package.k       KCL unit tests for the schema + defaults
├── inputs.yaml          sample inputs
└── README.md
```

## What's new vs 01

- **Two sources in one Package.** Both are `helm.template(...)` calls returning resource lists; `resources = [*_pg, *_app]` aggregates them.
- **Cross-source wiring by convention.** The webapp references the Postgres Secret by its predictable CloudNativePG name (`${appName}-pg-app`). No value needs to flow between the two source calls — both derive their values from `input`.
- **`postRenderer`.** A KCL lambda that post-processes every rendered resource in the webapp source. Used here to stamp a `team` label onto everything the app chart emits.
- **Unit tests.** `test_package.k` asserts schema defaults and validates that `check:` blocks catch the invariant violations.

## Run

```sh
akua add                                 # resolve cnpg + webapp charts
akua render --inputs inputs.yaml         # render both into ./rendered/
akua test                                # run test_package.k
```

## The cross-source convention pattern

CloudNativePG (and most mature Kubernetes operators) publishes contracts on resource naming — cluster `foo` creates Secret `foo-app` with key `uri`. That's a runtime contract. The webapp references it by the same convention at render time:

```python
env = [{
    name = "DATABASE_URL"
    valueFrom.secretKeyRef = {
        name = "${input.appName}-pg-app"   # CNPG convention
        key  = "uri"
    }
}]
```

If CNPG ever changed its naming convention, this is the one place we'd update — still at CI time, still deterministic. No `cluster.get()` runtime call ever needed.

## What's disallowed

- **Source A cannot reference Source B's output.** Both derive from `input`; cross-source late-binding is the RGD case. If you genuinely need it, route that source to a `ResourceGraphDefinition` output and let kro reconcile. See [06-multi-engine/](../06-multi-engine/) for the pattern.
- **No runtime cluster reads from KCL.** Determinism is load-bearing ([design-notes.md §2.2](../../docs/design-notes.md)).

## See also

- [package-format.md §4 Body](../../docs/package-format.md) — engine calls, postRenderer, aggregation
- [package-format.md §8 What's disallowed](../../docs/package-format.md) — cross-source wiring rules
- [03-multi-env-app/](../03-multi-env-app/) — next example: Package + App + Environment in one workspace
