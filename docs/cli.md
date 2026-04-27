# akua CLI reference

Complete reference for the `akua` binary. Every verb, every subcommand, every flag.

For the universal contract every verb honors (JSON output, exit codes, idempotency, plan mode, timeouts), see [cli-contract.md](cli-contract.md).

> **Status marker.** Sections marked ✅ describe verbs available in the shipping binary. Sections marked 🚧 describe verbs from the target surface that aren't wired yet. If a verb isn't marked, assume 🚧.
>
> **Shipped today (26 verbs):**
> `init` · `whoami` · `version` · `verify` · `render` · `add` · `dev` · `test` · `tree` · `pull` · `publish` · `sign` · `update` · `lock` · `push` · `repl` · `pack` · `remove` · `diff` · `check` · `inspect` · `lint` · `fmt` · `cache` · `auth` · `export`
>
> Run `akua --help` at the command line for the authoritative live list.

---

## Top-level flags

These flags are accepted by every verb:

| flag | description |
|---|---|
| `--json` | emit structured JSON to stdout |
| `--plan` | compute what the command would do; do not write |
| `--timeout=<duration>` | max time before exit 6 (e.g. `30s`, `5m`) |
| `--idempotency-key=<uuid>` | safe-retry key for write operations |
| `--log=<text\|json>` | stderr log format (default: text) |
| `--log-level=<debug\|info\|warn\|error>` | filter logs |
| `--verbose` / `-v` | more detail in logs |
| `--help` / `-h` | help for this verb |
| `--describe --json` | machine-readable spec of this verb |
| `--no-color` | disable terminal colors (implicit under `--json`) |
| `--no-interactive` | never block on stdin; fail with exit 1 if input is missing (implicit in agent context) |
| `--no-agent-mode` | disable agent-context auto-detection for this invocation |

### Agent-context auto-detection

When `akua` is run inside an AI-agent session, it detects this from env vars and auto-enables `--json`, `--log=json`, `--no-color`, `--no-progress`, and `--no-interactive`. Detection is keyed off `AGENT=<name>` (standard), `CLAUDECODE`, `GEMINI_CLI`, `CURSOR_CLI`, or `AKUA_AGENT`. Explicit flags always override detection.

```sh
# Human shell — text output
$ akua render
[pretty text output]

# Agent context — auto-JSON, no flag needed
$ CLAUDECODE=1 akua render
{"format":"raw-manifests","target":"./deploy","manifests":3,"hash":"sha256:…"}
```

See [cli-contract.md §1.5](cli-contract.md#15-agent-context-auto-detection) for the full detection rules, override semantics, and env-var reference.

---

## Verb index

```
AUTHOR              PUBLISH             DEPLOY              OPERATE
------              -------             ------              -------
akua init           akua attest         akua deploy         akua secret
akua add            akua publish        akua rollout        akua policy
akua render         akua pull           akua dev            akua audit
akua diff           akua inspect                            akua query
akua export                                                 akua infra

DEVELOP             SESSION             META
-------             -------             ----
akua test           akua login          akua help
akua fmt            akua logout         akua version
akua lint           akua whoami         akua telemetry
akua check                              akua lint-cli
akua bench
akua trace
akua cov
akua repl
akua eval
```

Thirty verbs. Grouped by purpose. Each covered below.

> **Quick disambiguation — `render` vs `export` vs `inspect` vs `diff`:**
>
> | verb | takes | produces | invokes engines? |
> |---|---|---|---|
> | `render` | Package + inputs | deploy-ready manifests | yes |
> | `export` | any canonical artifact | format view (JSON Schema, YAML, OpenAPI, Rego bundle) | no |
> | `inspect` | a published package ref | audit report (schema, sources, signatures, attestation) | no |
> | `diff` | two package refs | structural diff between them | no |
>
> When in doubt: `render` = "run the program"; `export` = "convert the format"; `inspect` = "audit what's there"; `diff` = "compare two versions."

---

## `akua init` ✅

Scaffold a new package or workspace.

```
akua init [name] [flags]
```

Creates a directory with:
- `package.k` — typed KCL Package definition
- `inputs.example.yaml` — sample input
- `.akua/` — metadata + lockfile location
- `README.md` — minimal docs stub

### Flags

| flag | description |
|---|---|
| `--template=<name>` | use a template (see `akua init --list-templates`) |
| `--package-name=<name>` | name for the Package (defaults to directory name) |
| `--no-git` | skip `git init` |
| `--list-templates` | list available templates |

### Templates

- `app` — single-service app (default)
- `app-with-db` — app + managed Postgres
- `umbrella` — multi-service composition
- `platform-std` — platform-team-published reusable package
- `empty` — bare package.k with a minimal schema

### Exit codes

0 success, 1 if target directory exists and is non-empty.

### JSON output

```json
{
  "name": "my-pkg",
  "path": "/absolute/path/my-pkg",
  "template": "app",
  "files": ["package.k", "inputs.example.yaml", ".akua/", "README.md"]
}
```

---

## `akua add` ✅

Add a dependency, chart, or source to the current package.

```
akua add <kind> <ref> [flags]
```

Kinds:
- `chart` — Helm chart (OCI or HTTP)
- `rgd` — kro ResourceGraphDefinition
- `kcl` — another KCL package
- `kustomize` — Kustomize base
- `app` — convenience: scaffold a user-authored App document referencing an existing Package

### Examples

```sh
akua add chart oci://ghcr.io/cloudnative-pg/charts/cluster --version 0.20.0
akua add kcl oci://ghcr.io/kcl-lang/k8s --version 1.31.2
akua add rgd ./local/glue.rgd.yaml
akua add app oci://pkg.akua.dev/node-api:3.2 --name my-api
```

For `chart` and `rgd`: generates a typed KCL subpackage under `./sources/<name>/` with `chart.k`, `values.schema.k`, and cached artifacts.

### Flags

| flag | description |
|---|---|
| `--name=<name>` | local alias (default: derived from ref) |
| `--version=<version>` | pin to specific version |
| `--registry=<url>` | override default registry |
| `--no-generate-schema` | skip schema generation |
| `--schema-source=<auto\|values-yaml\|url\|chart-path\|local>` | schema generation strategy |

### Exit codes

0 success, 1 user error, 2 system error (fetch failed), 4 rate limited.

### JSON output

```json
{
  "kind": "chart",
  "name": "cnpg-cluster",
  "ref": "oci://ghcr.io/cloudnative-pg/charts/cluster",
  "version": "0.20.0",
  "digest": "sha256:abc123…",
  "schema": "generated",
  "files_added": ["sources/cnpg-cluster/chart.k", "sources/cnpg-cluster/values.schema.k"]
}
```

---

## `akua lint` ✅

Parse-only check of a `package.k` — catches syntax errors and import-
resolution failures without executing the program. Runtime errors
(schema validation, unresolved options, engine failures) surface
through `akua render --dry-run`.

```
akua lint [flags]
```

### Flags

| flag | description |
|---|---|
| `--package=<path>` | path to the `package.k` file (default `./package.k`) |

### Exit codes

0 clean, 1 parse errors (or user error), 2 system error.

### JSON output

```json
{
  "status": "ok",
  "issues": []
}
```

Or on parse failure:

```json
{
  "status": "fail",
  "issues": [
    {
      "level": "error",
      "code": "Error(InvalidSyntax)",
      "message": "invalid token '!', consider using 'not '",
      "file": "/abs/path/package.k",
      "line": 2,
      "column": 2
    }
  ]
}
```

> **Planned expansion (🚧).** The target surface also checks
> Regal-style Rego lints, policy-tier compatibility, cross-engine
> reference integrity, and offers `--fix` auto-format integration.
> Lands with the policy pipeline (Phase C).

---

## `akua render` ✅

**Run the Package's program.** Evaluate the KCL, invoke every source engine (Helm, kro, Kustomize), compose results, produce deploy-ready manifests.

```
akua render [path] [flags]
```

**Discovery.** With no `path`, renders every user-authored document in the workspace whose schema declares render semantics — typically the workspace's App-shaped documents that reference a Package and carry inputs. With a `path`, renders only that file. Users author their own App / Environment / etc. schemas (akua does not specify them; see [package-format.md](package-format.md)); `render` processes whichever documents the workspace declares as renderable.

> **Not the same as `akua export`.** `render` executes the full pipeline against customer inputs and writes manifests a reconciler applies to a cluster. `export` converts a canonical artifact (schema, user-authored KCL document, policy bundle) into a format view (JSON Schema, YAML, OpenAPI, Rego bundle). Render needs inputs; export usually doesn't. Render invokes engines; export is format translation. See [`akua export`](#akua-export) below.

### Flags

| flag | description |
|---|---|
| `--package=<path>` | path to the `package.k` file (default `./package.k`) |
| `--inputs=<file>` | inputs file (JSON or YAML). When omitted, probes `./inputs.yaml` then `./inputs.example.yaml` next to the package; falls back to schema defaults if neither exists |
| `--out=<dir>` | write to directory (default: `./deploy/`) |
| `--stdout` | print rendered manifests as multi-doc YAML to stdout instead of writing files |
| `--dry-run` | render but don't write files |

> **Engine callables.** `pkg.render(Render)` (pure-Rust Package-of-Packages composition — see [`examples/08-pkg-compose`](../examples/08-pkg-compose)), `helm.template(Template)`, and `kustomize.build(Build)` all ship with embedded WASM backends. akua never shells out to `helm` or `kustomize` binaries — every engine runs inside the wasmtime sandbox alongside the render worker; see [`docs/security-model.md`](security-model.md) and [`docs/embedded-engines.md`](embedded-engines.md).
>
> **One render output.** akua writes raw YAML manifests, one file per resource. Distribution shapes like Helm charts or OCI bundles are future `akua publish --as <format>` concerns — they wrap rendered manifests at distribution time, not as a Package-declared output.

### Exit codes

0 success, 1 user error, 2 system error. (Phase B adds 3 for policy deny.)

### JSON output

```json
{
  "format": "raw-manifests",
  "target": "./deploy",
  "manifests": 1,
  "hash": "sha256:…",
  "files": ["000-configmap-hello.yaml"]
}
```

`format` is always `"raw-manifests"` today. `target` is the resolved output directory. `hash` is `sha256:<hex>` of the concatenated `<filename>\n<yaml>` blocks — stable across runs when inputs + lockfile + akua version match.

---

## `akua diff` ✅

Structural diff between two package versions, or between a local package and a published version.

```
akua diff <a> <b> [flags]
akua diff <ref>                    # diff local HEAD against published ref
```

### Flags

| flag | description |
|---|---|
| `--format=<structural\|yaml\|both>` | diff level (default: structural) |
| `--scope=<schema\|sources\|manifests\|all>` | what to compare (default: all) |
| `--filter=<pattern>` | only show diffs matching pattern |

### Exit codes

0 if no structural changes, 1 if changes present. Useful for CI gates: non-zero = upgrade is breaking.

### JSON output

```json
{
  "schema": {
    "added": ["adminEmail"],
    "removed": [],
    "type_changed": [],
    "default_changed": [{"path": "replicas", "from": 3, "to": 5}]
  },
  "sources": {
    "added": [],
    "removed": [],
    "version_changed": [{"name": "cnpg", "from": "0.20.0", "to": "0.21.0"}]
  },
  "manifests": {
    "added": 2,
    "removed": 0,
    "modified": 4
  },
  "policy_compat": "allow"
}
```

---

## `akua attest` 🚧

Emit a SLSA v1 provenance predicate for the current package or a built artifact.

```
akua attest [path] [flags]
```

### Flags

| flag | description |
|---|---|
| `--key=<cosign-key-ref>` | cosign signing key |
| `--oci=<ref>` | attest a remote OCI artifact instead of local build |
| `--out=<file>` | where to write the predicate (default: `<target>.attestation.json`) |
| `--format=<slsa-v1\|in-toto>` | predicate format (default: slsa-v1) |

### JSON output

```json
{
  "subject": {
    "name": "pkg.akua.dev/payments-api",
    "digest": "sha256:…"
  },
  "predicateType": "https://slsa.dev/provenance/v1",
  "predicate": { /* SLSA v1 predicate */ },
  "signed": true,
  "signature": "./attestation.sig"
}
```

---

## `akua publish` 🚧

Push a signed package to an OCI registry.

```
akua publish [path] [flags]
```

### Flags

| flag | description |
|---|---|
| `--to=<oci-ref>` | destination (default: `[package].spec.publish.default`) |
| `--tag=<tag>` | tag (default: `[package].version`) |
| `--sign` | sign with configured cosign key (default: on if logged in) |
| `--attest` | emit and attach SLSA predicate (default: on) |
| `--public` | mark as public (required for ghcr public visibility) |

### Exit codes

0 success, 1 user error, 2 system error, 3 policy deny, 4 rate limited, 5 needs approval.

### JSON output

```json
{
  "package": "pkg.akua.dev/payments-api",
  "version": "3.2.0",
  "digest": "sha256:…",
  "signed": true,
  "attestation_digest": "sha256:…",
  "size_bytes": 1045832,
  "upload_duration_ms": 1823
}
```

---

## `akua pull` 🚧

Fetch a package from an OCI registry into the local cache.

```
akua pull <ref> [flags]
```

### Flags

| flag | description |
|---|---|
| `--verify` | verify cosign signature (default: on) |
| `--unpack=<dir>` | unpack to directory instead of caching |
| `--insecure` | allow unsigned / unverifiable (dangerous) |

---

## `akua inspect` ✅

Report a `package.k`'s input surface — every `option()` call-site with
its name, declared type, required flag, default, and help text.
Parse-only: the program is not executed.

```
akua inspect [flags]
```

### Flags

| flag | description |
|---|---|
| `--package=<path>` | path to the `package.k` file (default `./package.k`) |

### Exit codes

0 success, 1 user error (missing file, parse failure), 2 system error.

### JSON output

```json
{
  "path": "./package.k",
  "options": [
    {
      "name": "input",
      "required": false
    }
  ]
}
```

Each option carries `name`, `required`, and optionally `type`,
`default`, `help` when the KCL source supplies them. `type` is
currently empty for the canonical `input: Input = ctx.input()` form —
kcl_lang's `list_options` only reads a type arg passed directly to
`option()`; full binding-context recovery arrives with AST walking.

> **Planned expansion (🚧).** The target surface also audits a
> *published* OCI package — signatures, SLSA attestation chain, chart
> sources, rendered-manifest counts — via `akua inspect <oci://…>`.
> That depends on the OCI fetch pipeline (Phase B/C).

---

## `akua export` ✅

**Convert a Package's `Input` schema to a standard interchange format.** Emits JSON Schema 2020-12 (raw) or OpenAPI 3.1 (Input wrapped under `components.schemas`). Backed by KCL's resolver + AST walk; field docstrings become `description`, `@ui(...)` decorators become `x-ui` extensions.

```
akua export --package <path> [--format=<json-schema|openapi>] [--out=<file>]
```

> **Not the same as `akua render`.** `export` is format translation — it doesn't invoke Helm / kro / Kustomize and doesn't need customer inputs. It answers *"how do I describe this Package's inputs in a format other tools understand?"* Use `render` when you want deploy-ready manifests; use `export` when you want a schema for a UI form renderer or API doc generator. See [`akua render`](#akua-render) above.

### Supported formats

| format | output | for |
|---|---|---|
| `json-schema` (default) | JSON Schema Draft 2020-12 for the `Input` schema | install UIs, form renderers (rjsf, JSONForms) |
| `openapi` | OpenAPI 3.1 with `Input` under `components.schemas` | API docs (Swagger UI, Redoc), client SDK generation, admission-webhook validators |

### Flags

| flag | description |
|---|---|
| `--package=<path>` | path to `package.k` (default `./package.k`) |
| `--format=<fmt>` | `json-schema` (default) or `openapi` |
| `--out=<file>` | write to file (default: stdout) |

### `@ui(...)` decorators → `x-ui` extension

`@ui(...)` keyword arguments on schema attributes are projected onto the JSON Schema property as the OpenAPI-3.1-compliant `x-ui` extension. Renderers that recognise it (rjsf, custom form UIs) consume the hints; renderers that don't, ignore them.

```kcl
schema Input:
    @ui(order=10, group="Identity")
    name: str = "hello"

    @ui(order=20, widget="slider", min=1, max=20)
    replicas: int = 2
```

```json
{
  "properties": {
    "name": {
      "type": "string",
      "default": "hello",
      "x-ui": {"order": 10, "group": "Identity"}
    },
    "replicas": {
      "type": "integer",
      "default": 2,
      "x-ui": {"order": 20, "widget": "slider", "min": 1, "max": 20}
    }
  }
}
```

### Examples

```sh
# JSON Schema for a web form
akua export --package package.k > inputs.schema.json

# OpenAPI 3.1 for API docs
akua export --package package.k --format=openapi > package.openapi.json

# Write to file directly
akua export --package package.k --out=exported/inputs.schema.json
```

### Exit codes

0 success; 1 if `package.k` lacks an `Input` schema or has KCL syntax errors; 5 on filesystem errors.

---

## `akua dev` ✅

Start the hot-reload development loop.

```
akua dev [flags]
```

Single long-running process. Watches workspace for changes. Renders, validates policy, applies to local target. Serves a browser UI at `http://localhost:5173`.

### Flags

| flag | description |
|---|---|
| `--target=<local\|dry-run\|cluster:<name>>` | apply target (default: local kind cluster) |
| `--port=<num>` | browser UI port (default: 5173) |
| `--policy=<tier>` | policy tier for live checks (default: `tier/dev`) |
| `--no-browser` | don't open browser automatically |
| `--fresh` | wipe persistent state before starting |
| `--inputs=<file>` | override inputs file |

### Exit codes

0 on clean shutdown (Ctrl-C), 1 for startup errors.

### JSON output (when `--json`)

Streaming JSON-lines of pipeline events:

```
{"t":1713636000,"stage":"render","app":"api","duration_ms":127,"status":"ok"}
{"t":1713636001,"stage":"policy","resource":"Deployment/api","verdict":"allow"}
{"t":1713636001,"stage":"apply","resource":"Deployment/api","op":"patch","duration_ms":198}
{"t":1713636002,"stage":"reconcile","resource":"Deployment/api","status":"ready"}
```

Useful for agents that want to drive `akua dev` programmatically.

---

## `akua deploy` 🚧

Deploy rendered output to a reconciler target.

```
akua deploy [path] [flags]
```

Depending on `--to=<target>`:

- `--to=argo` — render, open a PR against the deploy repo, Argo picks up
- `--to=flux` — same with Flux
- `--to=kro` — deploy the RGD output to kro
- `--to=helm` — `helm upgrade --install`
- `--to=kubectl` — `kubectl apply` directly
- `--to=<custom-driver>` — configured driver

### Subcommands

```
akua deploy status   --handle=<h>
akua deploy wait     --handle=<h> [--timeout=<d>]
akua deploy rollback --change=<id>
akua deploy history  [--service=<name>] [--last=<n>]
akua deploy cancel   --handle=<h>
```

### JSON output (main verb)

```json
{
  "handle": "r-4f2c9a",
  "target": "argo",
  "status": "pending",
  "resources_planned": 12,
  "pr_url": "https://github.com/acme/deploy-repo/pull/48",
  "policy": { "verdict": "allow" }
}
```

### JSON output (status)

```json
{
  "handle": "r-4f2c9a",
  "phase": "reconciling",
  "health": "degraded",
  "ready": 2,
  "total": 3,
  "started_at": "2026-04-20T14:03:00Z",
  "last_event": "Deployment/api: 2/3 replicas ready"
}
```

---

## `akua rollout` 🚧

Cross-repo / cross-service staged rollout orchestration.

```
akua rollout <spec> [flags]
```

Where `<spec>` is a user-authored rollout document (KCL or YAML view) or OCI ref.

### Subcommands

```
akua rollout plan    <spec>           # show planned stages without executing
akua rollout apply   <spec>           # execute the rollout
akua rollout status  --handle=<h>
akua rollout pause   --handle=<h>
akua rollout resume  --handle=<h>
akua rollout abort   --handle=<h>     # triggers rollback
```

### Flags

| flag | description |
|---|---|
| `--strategy=<parallel\|staged\|canary>` | override rollout strategy |
| `--batch-size=<n>` | override parallel batch size |
| `--soak=<duration>` | soak time between stages |

---

## `akua secret` 🚧

Typed secret operations. Secrets move as refs, never raw bytes.

```
akua secret <sub> [args]
```

### Subcommands

```
akua secret add     <name> --from-env=<var> --store=<vault|infisical|sops>
akua secret get     <name> --format=ref       # returns a ref; never raw value
akua secret rotate  <name>
akua secret grant   <name> --to=<service> --scope=<read|write>
akua secret revoke  <name> --from=<service>
akua secret trace   <name>                    # who has access, who's used it
akua secret list    [--store=<name>]
akua secret delete  <name>                    # soft delete; needs approval
```

### JSON output (trace)

```json
{
  "name": "stripe-api-key",
  "store": "vault",
  "ref": "vault://secrets/stripe/api-key",
  "grants": [
    {"service": "checkout", "scope": "read", "granted_at": "2026-01-15"}
  ],
  "last_access": "2026-04-20T14:03:00Z",
  "rotation": {
    "policy": "30d",
    "last_rotated": "2026-04-15",
    "next_due": "2026-05-15"
  }
}
```

---

## `akua policy` 🚧

Policy tier operations.

```
akua policy <sub> [args]
```

### Subcommands

```
akua policy check   [--tier=<name>] [--target=<file-or-dir>]
akua policy tiers                                     # list available tiers
akua policy show    <tier>                            # display a tier's rules
akua policy diff    <tier-a> <tier-b>
akua policy install <tier> [--from=<oci-ref>]
akua policy fork    <tier> --as=<new-name>
akua policy publish <tier>                            # publish custom tier to OCI
```

### JSON output (check)

```json
{
  "tier": "tier/production",
  "verdict": "allow" | "deny" | "needs-approval",
  "checks": {
    "resource_limits":    "pass",
    "non_privileged":     "pass",
    "readiness_probes":   "pass",
    "budget_caps":        "warn"
  },
  "failing": [
    {
      "rule": "budget_cap",
      "resource": "Deployment/api",
      "reason": "replicas * resources.requests.cpu exceeds team budget",
      "suggested_fix": "reduce replicas to 3 or increase budget to $500/mo"
    }
  ],
  "approvers": ["@team/platform"]
}
```

---

## `akua audit` 🚧

Causality spine. Trace changes, explain incidents, query the audit trail.

```
akua audit <sub> [args]
```

### Subcommands

```
akua audit explain   <change-id-or-incident-id>
akua audit trace     --resource=<name> [--since=<duration>]
akua audit search    --actor=<name> [--action=<verb>]
akua audit export    --format=<json|csv> --since=<time> --until=<time>
akua audit who       <resource>                       # who has permission to modify
```

### JSON output (explain)

```json
{
  "incident_id": "i-47",
  "trigger": {
    "type": "error_rate_spike",
    "service": "checkout",
    "at": "2026-04-20T14:08:00Z"
  },
  "root_cause": {
    "change_id": "c-4f2c9a",
    "actor": "agent-experiments-4",
    "reason": "enabled new flag X",
    "committed_at": "2026-04-20T14:03:00Z"
  },
  "resolution": {
    "action": "rollback",
    "change_id": "c-9b3",
    "actor": "agent-incident-responder",
    "completed_at": "2026-04-20T14:10:00Z"
  },
  "duration_minutes": 7,
  "learned": "experiment should gate on p99 budget; see policy-template/experiment-v2"
}
```

---

## `akua query` 🚧

Structured queries against observability stores.

```
akua query <expr> [flags]
```

Query syntax: promql-like for metrics, logql-like for logs, tempoql for traces. Returns JSON.

### Flags

| flag | description |
|---|---|
| `--backend=<prometheus\|loki\|tempo\|auto>` | which store |
| `--since=<duration>` | time window (default: 1h) |
| `--format=<json\|table\|chart>` | output shape |

### Example

```sh
akua query "error_rate p99 last 1h service=checkout" --json
```

```json
{
  "query": "error_rate p99 last 1h service=checkout",
  "backend": "prometheus",
  "result": {
    "value": 0.023,
    "baseline": 0.001,
    "change_pct": 2200,
    "samples": 60
  }
}
```

---

## `akua infra` 🚧

Cluster, network, DNS, cert primitives. Wraps Crossplane or Terraform under the hood.

```
akua infra <sub> [args]
```

### Subcommands

```
akua infra plan   <file>
akua infra apply  <file>
akua infra status
akua infra drift                    # show drift between desired and observed
akua infra import <resource>        # bring external resource under management
```

---

## `akua login` 🚧

Authenticate to OCI registries and signing providers.

```
akua login [registry] [flags]
```

### Examples

```sh
akua login                              # interactive; logs into akua.dev
akua login ghcr.io                      # interactive; token prompt
akua login ghcr.io --token=$GITHUB_PAT  # scripted
```

Credentials are stored in the system credential store (Keychain, libsecret, Credential Manager). Never plaintext.

---

## `akua logout` 🚧

Remove stored credentials.

```
akua logout [registry]
akua logout --all
```

---

## `akua whoami` ✅

Display current identity, logged-in registries, and scopes.

```
akua whoami [flags]
```

### JSON output

```json
{
  "identity": "user@example.com",
  "registries": [
    {"url": "ghcr.io", "user": "robin", "expires_at": null},
    {"url": "akua.dev", "user": "robin", "tier": "team", "expires_at": "2026-05-20"}
  ],
  "scopes": ["packages:write", "policy:read"],
  "agent_context": {
    "detected": true,
    "agent": "claude-code",
    "source_env": "CLAUDECODE"
  }
}
```

`agent_context` is present when akua auto-detected an agent session (see [cli-contract.md §1.5](cli-contract.md#15-agent-context-auto-detection)). When no agent is detected, the field is `{"detected": false}`.

---

## `akua test` 🚧

Run unit tests for packages, policies, or both. Unified test runner across engines — detects target types by file extension.

```
akua test [path] [flags]
```

Discovers and runs:

- `**/*_test.rego` — Rego policy tests via embedded OPA
- `**/*_test.k` / `test_*.k` — KCL test files via embedded KCL
- Kyverno `test.yaml` bundle tests (when the bundle is imported)
- Golden-output tests (`*.golden.yaml` compared against current render)

### Flags

| flag | description |
|---|---|
| `--coverage` | emit per-rule / per-schema coverage report |
| `--watch` | re-run on file change |
| `--golden` | enable / verify golden-output comparisons |
| `--filter=<regex>` | run only matching tests |
| `--timeout=<dur>` | per-test timeout (default 30s) |
| `--engine=<auto\|embedded\|shell>` | engine selection (see [embedded-engines.md](embedded-engines.md)) |

### Exit codes

0 if all pass, 1 if any fail, 2 on infrastructure error.

### JSON output

```json
{
  "summary": { "passed": 24, "failed": 1, "skipped": 2, "duration_ms": 413 },
  "results": [
    {
      "file":     "policies/production_test.rego",
      "test":     "test_deny_missing_team_label",
      "status":   "pass",
      "duration_ms": 12
    },
    {
      "file":     "packages/api/test_api.k",
      "test":     "test_default_replicas",
      "status":   "fail",
      "message":  "expected replicas=3, got 1",
      "duration_ms": 8
    }
  ],
  "coverage": { "overall": 0.72, "by_rule": { "deny_budget_exceeded": 0.0 } }
}
```

---

## `akua fmt` ✅

Format KCL and Rego sources in place.

```
akua fmt [path] [flags]
```

Uses embedded `kcl fmt` for `.k` files and embedded `opa fmt` for `.rego` files. Idempotent; safe to run in CI.

### Flags

| flag | description |
|---|---|
| `--check` | exit 1 if anything would change (CI gate); do not modify files |
| `--diff` | print unified diff of changes without applying |

### Exit codes

0 success, 1 formatting needed (with `--check`), 2 parse error.

---

## `akua check` ✅

Syntax + type + dependency check. No execution, no rendering. Fast.

```
akua check [path] [flags]
```

Stricter than `akua lint` (actual compile errors, not style); cheaper than `akua render` (doesn't invoke engines). Good for IDE save hooks and pre-commit.

### JSON output

```json
{
  "valid": true,
  "summary": { "files": 12, "errors": 0, "warnings": 0, "duration_ms": 89 }
}
```

On error:

```json
{
  "valid": false,
  "errors": [
    {
      "file":  "package.k",
      "line":  14,
      "code":  "E_SCHEMA_INVALID",
      "message": "expected int, got string",
      "suggestion": "remove quotes around value"
    }
  ]
}
```

---

## `akua bench` 🚧

Benchmark policy evaluation and package render latency.

```
akua bench [path] [flags]
```

Uses OPA partial evaluation for policy benchmarks; the KCL interpreter's own timing for package render. Intended for high-throughput evaluators (admission webhooks, CI gates at scale).

### Flags

| flag | description |
|---|---|
| `--iterations=<n>` | run each benchmark N times (default 1000) |
| `--input=<file>` | use this input for the benchmark (default: workspace defaults) |
| `--engine=<auto\|embedded\|shell>` | engine selection |

### JSON output

```json
{
  "benchmarks": [
    {
      "name":            "tier/production:deny",
      "iterations":      1000,
      "total_ms":        47,
      "mean_us":         47,
      "p99_us":          82,
      "rules_evaluated": 47
    }
  ]
}
```

---

## `akua trace` 🚧

Explain the evaluation path of a policy query. Useful for debugging "why did this rule deny?" or "why didn't this rule fire?"

```
akua trace <query> [flags]
```

Passes through OPA's `--explain` with structured output.

### Flags

| flag | description |
|---|---|
| `--input=<file>` | input document for the query |
| `--depth=<notes\|fails\|full\|debug>` | trace verbosity (default fails) |
| `--data=<dir>` | policy bundle directory (default: current workspace) |

### Example

```sh
$ akua trace 'data.akua.policies.production.deny' --input=./deploy/api.yaml
```

```
EVAL  data.akua.policies.production.deny
  EVAL  input.resource.kind == "Deployment"            TRUE
  EVAL  not input.resource.metadata.labels["team"]      TRUE
  EVAL  msg := "production Deployments must have a team label"
ALLOW deny[msg] evaluated to {"production Deployments must have a team label"}
```

---

## `akua cov` 🚧

Generate a test coverage report across rules (Rego) and schemas (KCL).

```
akua cov [path] [flags]
```

Equivalent to `akua test --coverage` but produces a standalone report. Useful for CI gates that enforce a minimum coverage percentage.

### Flags

| flag | description |
|---|---|
| `--min=<percentage>` | fail if coverage is below threshold (e.g. `--min=80`) |
| `--format=<json\|html\|lcov>` | report format (default json) |

---

## `akua repl` ✅

Interactive REPL for exploring policies and packages.

```
akua repl [flags]
```

Supports two modes (tab-switched):

- **Rego mode** — runs against the current policy set; evaluates expressions, shows trace, imports any `data.akua.policies.*`
- **KCL mode** — runs against the current package; evaluates expressions, shows schema types, hot-imports modules

Useful for experimenting before committing to a rule or package change.

---

## `akua eval` 🚧

One-shot evaluator — cheap, scriptable. For Rego queries and KCL expressions without entering the REPL.

```
akua eval <query> [flags]
akua eval --lang=rego 'data.akua.policies.production.deny'
akua eval --lang=kcl  'schema Input; input = Input {...}; input.replicas * 2'
```

### Flags

| flag | description |
|---|---|
| `--lang=<rego\|kcl>` | expression language (default: inferred from query syntax) |
| `--input=<file>` | input document (Rego) or values file (KCL) |
| `--data=<dir>` | policy / package bundle |

### JSON output

```json
{
  "lang": "rego",
  "query": "data.akua.policies.production.deny",
  "result": ["production Deployments must have a team label"],
  "duration_ms": 5
}
```

---

## `akua help` 🚧

```
akua help                    # list all verbs
akua help <verb>             # detailed help for one verb
akua help --json             # machine-readable command tree
```

The `--json` form is the agent-discovery surface.

---

## `akua version` ✅

```
akua version                 # print version + git SHA
akua version --json
```

```json
{
  "version": "0.1.0",
  "commit": "abc123",
  "build_date": "2026-04-20",
  "go_version": "1.22",
  "rust_version": "1.82",
  "kcl_plugin_version": "0.1.0"
}
```

---

## `akua telemetry` 🚧

Opt-in, anonymized usage data.

```
akua telemetry status
akua telemetry enable
akua telemetry disable
akua telemetry show              # print last 100 records that WOULD be sent
```

Default: disabled. Agents enable explicitly if desired.

---

## `akua lint-cli` (internal, advanced) 🚧

Validate that the current binary honors the CLI contract.

```
akua lint-cli
```

Used in CI to catch contract violations before release.

---

## Environment variables

A minimal set. No hidden state.

### akua-specific

| var | purpose |
|---|---|
| `AKUA_REGISTRY` | default OCI registry for publish/pull |
| `AKUA_CACHE_DIR` | override cache location (default: `$XDG_CACHE_HOME/akua`) |
| `AKUA_LOG_LEVEL` | override `--log-level` |
| `AKUA_NO_TELEMETRY` | force telemetry off (for CI) |
| `AKUA_TOKEN_FILE` | path to a token file for non-interactive auth |
| `AKUA_AGENT` | signal an agent context explicitly (value is the agent name) |
| `AKUA_NO_AGENT_DETECT` | disable agent-context auto-detection |

All of these can be overridden by flags where a flag exists. Humans typically set nothing; agents typically set nothing (their environment already identifies them).

### Agent-context env vars (detected, never written)

These are set by agent runtimes, not by akua. akua reads them to determine whether it's running in an agent context.

| var | set by |
|---|---|
| `AGENT=<name>` | Goose (`goose`), Amp (`amp`), Codex (`codex`), Cline (`cline`), OpenCode (`opencode`) — emerging standard |
| `CLAUDECODE=1` | Claude Code |
| `GEMINI_CLI=1` | Gemini CLI |
| `CURSOR_CLI=1` | Cursor CLI |
| `GOOSE_TERMINAL=1`, `AMP_THREAD_ID=<id>`, `CODEX_SANDBOX=<id>`, `CLINE_ACTIVE=true` | secondary identifiers per agent — recorded as context |

See [cli-contract.md §1.5](cli-contract.md#15-agent-context-auto-detection) for detection rules and precedence.

---

## Exit code reference (summary)

From [cli-contract.md](cli-contract.md):

| code | meaning |
|---|---|
| 0 | success |
| 1 | user error |
| 2 | system error |
| 3 | policy deny |
| 4 | rate limited |
| 5 | needs approval |
| 6 | timeout |

---

## Stability and versioning

- Pre-v1.0: breaking changes require a minor version bump + changelog entry.
- v1.0 onward: flag removal requires 6-month deprecation; exit code semantics never change.
- JSON output keys are part of the stability contract.
- New verbs can be added without bumping major.

---

## What's not in this reference

- Implementation details (Rust crate structure, KCL plugin ABI).
- The TypeScript SDK (see [sdk.md](sdk.md)).
- The CLI contract (see [cli-contract.md](cli-contract.md)).
- Examples of usage (see [examples/](../examples/)).
- Architecture (see [architecture.md](architecture.md)).

## Spec cross-references

- **Package format** — [package-format.md](package-format.md) (KCL Package, four regions, engine callables)
- **Policy format** — [policy-format.md](policy-format.md) (Rego as host, compile-resolved imports, custom builtins)
- **Lockfile** — [lockfile-format.md](lockfile-format.md) (`akua.toml` + `akua.lock`)
