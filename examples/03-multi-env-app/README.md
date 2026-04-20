# Example 03 — multi-env app

The shape most production workspaces actually use — but importantly, **akua does not specify any of it beyond `Package.k`**. This example shows how a team builds their own "App + Environment" vocabulary on top of akua's substrate.

The workspace defines its own KCL schemas under `schemas/` (its `App`, its `Environment`) — whatever fits this team's deployment reality. Apps and environments are then instances of those user-authored schemas, not instances of akua-owned kinds. Different teams will have different shapes; akua doesn't prescribe.

## Layout

```
03-multi-env-app/
├── akua.mod                       workspace root + shared deps
├── akua.sum                       digest + signature ledger
├── schemas/                       workspace-local schemas (NOT akua-specified)
│   ├── app.k                      this team's App shape
│   └── environment.k              this team's Environment shape
├── package/                       reusable Package (would be OCI-published)
│   └── package.k
├── apps/
│   └── checkout/
│       ├── dev.k                  one App per (app × env)
│       ├── staging.k
│       └── production.k
├── environments/
│   ├── dev.k
│   ├── staging.k
│   └── production.k
└── README.md
```

## Why one App per env instead of one App with a per-env map?

Because KCL is expressive enough to emit multiple resources, and because each (app × env) is an independent reconcilable unit. One file per deploy = one PR scope per change = one `App.status` to report on = one deploy to audit. This matches how Argo ApplicationSet and Flux Kustomizations already work in the ecosystem.

If the delta between envs is small, you can collapse to one file with a comprehension:

```python
import ....schemas.app as s
import ....package as pkg

_shared = pkg.Input { appName: "checkout", team: "payments" }
envs = {
    dev:        _shared | { hostname: "checkout.dev.example.com",     replicas: 1 }
    staging:    _shared | { hostname: "checkout.staging.example.com", replicas: 2 }
    production: _shared | { hostname: "checkout.example.com",         replicas: 5 }
}

[ s.App { name: "checkout-${e}", env: e, package: "oci://...", inputs: v,
          target: s.Target { reconciler: "argo", cluster: "${e}-eu" } }
  for e, v in envs ]
```

Same output. Pure KCL. No special akua flags, no akua-owned schema to obey.

## Render

```sh
akua add                            # resolve deps → writes akua.sum
akua render                         # renders every App document it finds
akua render --filter=env=production # narrow to one env using a general filter
```

There is no `--env` or `--all-envs` flag. `akua render` processes every document of a KCL-declared shape in the workspace. Filtering is a general-purpose concern expressed via `--filter` over any field, not an env-specific primitive.

## Deriving YAML views

Reconcilers consume YAML. The `.k` files are authoritative; the YAML view is derived on demand:

```sh
akua export apps/checkout/production.k --format=yaml > apps/checkout/production.yaml
akua export environments/production.k  --format=yaml > environments/production.yaml
```

Check these YAML files in or don't — they regenerate deterministically. The **rule**: never hand-edit the YAML — edit the `.k` and re-export.

## Flow for a change

1. Edit `apps/checkout/production.k` (e.g. bump `replicas` from 5 to 7).
2. CI runs `akua check && akua lint && akua test && akua render`.
3. `akua policy check --tier=tier/production` against the rendered manifests — returns `allow` / `deny` / `needs-approval`.
4. If `needs-approval`: the review surface notifies approvers; human approves.
5. PR merges; deploy repo gets updated YAML; Argo/Flux reconciles.

## Why this design

- **"Substrate, not content."** akua provides the typed authoring language (KCL), the signed distribution (Package + OCI), the deterministic pipeline, the policy host. Workspace concepts (App, Environment, whatever else) are workspace territory. That's what the CLAUDE.md invariant means in practice.
- **Enterprise reality.** Every org's deployment shape is subtly different — additional approver rules, per-env secret stores, SLO targets, blue/green weights, regulatory fields. A ship-your-own-Environment-kind approach would pick one shape and lose the rest. Letting users define their own schemas means nobody is forced through a shape that misses a field they need.
- **akua Cloud carries its own concepts.** If the commercial Cloud offering needs "Workspace", "Tenant", cross-workspace "Environment" — those are Convex schemas in the Cloud backend, not OSS KRMs. Keep the OSS surface small.

## See also

- [package-format.md](../../docs/package-format.md) — `Package.k`, the one akua-specified shape
- [04-policy-tier/](../04-policy-tier/) — authoring the `tier/production` policy the production environment references
- [design-notes.md](../../docs/design-notes.md) — why akua has no KRM vocabulary
