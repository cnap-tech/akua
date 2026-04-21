# Package format

The canonical shape of an akua Package. A Package is a reusable definition authored in **KCL** and published as a signed OCI artifact. `Package.k` is the only shape akua itself specifies; higher-level workspace concepts (App / Environment / Cluster / PolicySet / etc.) are user-defined KCL schemas in the consumer's workspace, not akua-owned kinds.

This document specifies what a `package.k` file may contain. Companion references: [lockfile-format.md](lockfile-format.md) for `akua.toml` / `akua.lock`, [policy-format.md](policy-format.md) for Rego.

---

## 1. Anatomy

Every Package is one KCL program with three typed regions:

```python
# package.k

# (1) imports — engines, schemas, reusable modules
import akua.helm
import charts.cnpg    as cnpg
import charts.webapp  as webapp

# (2) schema — the public input contract
schema Input:
    appName:  str
    hostname: str
    replicas: int = 3

input: Input = option("input") or Input {}

# (3) body — source-engine calls + transforms + aggregation
_pg  = helm.template(helm.Template { chart = cnpg.Chart,   values = ... })
_app = helm.template(helm.Template { chart = webapp.Chart, values = ... })

resources = _pg + _app
```

That's it. `akua render` writes `resources` as raw YAML files under
`--out`. Other distribution shapes (Helm charts, OCI bundles, kro RGDs)
come from either (a) **transformation** functions invoked in the body
that produce more K8s resources (`kro.rgd(...)`, `crossplane.composition(...)`),
or (b) future `akua publish --as <format>` at distribution time. The
Package itself never pre-commits to an emit format — `resources` is
the single canonical thing it produces.

---

## 2. Imports

An import brings one of three things into scope:

| import form | purpose | pinned by |
|---|---|---|
| `import akua.<engine>` | a source-engine callable (`helm`, `rgd`, `kustomize`, `oci`) | the akua CLI version |
| `import charts.<name>` | a typed source package previously added via `akua add` | `akua.toml` |
| `import <local/path>` | a local KCL module within this package | the filesystem |

Imports are resolved at build time against `akua.toml` (declared deps) and verified against `akua.lock` (digest + signature). Failed verification is a compile error. A missing pin is a compile error.

Engine callables live at `akua.*`:

- `akua.helm.template(chart, values, postRenderer?)` — Helm source
- `akua.rgd.instantiate(rgd_def, instance_spec)` — kro RGD source
- `akua.kustomize.build(path)` — Kustomize base
- `akua.oci.fetch_manifests(ref)` — pre-rendered OCI bundle

Every engine callable returns `[Resource]`, a typed list of Kubernetes-shaped resource dicts.

---

## 3. Schema — the public input contract

The `Input` schema declares what customers (or App resources) must provide to render this Package.

Rules:

- Must be named `Input`. The binding line is canonically:

  ```python
  input: Input = option("input") or Input {}
  ```

  `option("input")` reads the value the runtime provides (via KCL's `-D` flag or the equivalent in `ExecProgramArgs.args`). `or Input {}` falls back to schema defaults when nothing is supplied. This is the only non-obvious line in a Package — every Package uses it verbatim.

  The pattern is deliberate: with this binding the Package is **standalone-valid KCL** (runnable via `kcl package.k -D input='{"…"}'`), so `kcl fmt` / `kcl lint` / IDE LSPs all work on Packages without akua-specific preprocessing.
- Fields use KCL's native type syntax: `str`, `int`, `float`, `bool`, `[T]`, `{str: T}`, unions (`"a" | "b" | "c"`), nested schemas.
- Fields without defaults are required. Fields with defaults are optional.
- Use KCL docstrings for field documentation — `akua` tooling surfaces them in autocomplete and generated docs.
- `check:` blocks can express cross-field constraints; they run during `akua render`.
- No runtime side effects (no env lookups, no filesystem, no network). KCL's sandbox enforces this.

Example with all shapes:

```python
schema Input:
    """Public inputs for this package."""

    # Required, primitive
    appName: str

    # Required, nested schema
    routing: RoutingInput

    # Optional with default
    replicas: int = 3

    # Optional union
    tier: "startup" | "production" = "startup"

    # Optional list of nested schemas
    additional_hosts: [HostInput] = []

    # Optional dict
    labels: {str: str} = {}

    # Cross-field constraint
    check:
        replicas >= 1, "replicas must be at least 1"
        len(additional_hosts) < 10, "at most 10 additional hosts"

schema RoutingInput:
    hostname: str
    tls:      bool = True
    issuer:   str  = "letsencrypt-prod"

schema HostInput:
    hostname: str
    priority: int = 0
```

### UI hints (optional)

When a Package is consumed through a UI (merchant install form, Package Studio, generated Swagger form), renderers benefit from hints about field ordering, labels, placeholders, grouping. Without these, renderers fall back to heuristics (field-name casing, schema-declaration order).

akua reads UI hints from two sources:

**KCL docstrings** — the field's `"""…"""` docstring becomes the UI label and description:

```python
schema Input:
    """Public inputs for this package."""

    appName: str
    """Application name. Lowercase, hyphen-separated."""

    hostname: str
    """Public hostname. Must be a valid RFC 1123 DNS name."""

    replicas: int = 3
    """Number of replicas. Minimum 1 in production."""
```

Zero-effort for authors; zero new vocabulary. Docstrings serve double duty (human-readable + UI metadata).

**KCL schema decorators** — optional, for ordering / grouping / widget hints that docstrings can't carry:

```python
schema Input:
    @ui(order=10, group="Identity")
    appName: str

    @ui(order=20, group="Identity", placeholder="app.example.com")
    hostname: str

    @ui(order=30, group="Capacity", widget="slider", min=1, max=20)
    replicas: int = 3
```

Decorators are a future-compatible layer — they're ignored by the KCL compiler proper and consumed by `akua export` when generating UI-renderable views.

### Exporting a view vs rendering

The canonical Package is KCL. Two different verbs produce different outputs from it:

| verb | purpose | needs inputs? | output |
|---|---|---|---|
| `akua export` | convert the Package's schema / metadata to a standard format for external tools | no | JSON Schema / OpenAPI / YAML / Rego bundle |
| `akua render` | execute the Package's full pipeline and produce deploy-ready Kubernetes manifests | yes | rendered YAML the reconciler applies |

For install UIs, API docs, rjsf / JSONForms, admission webhook schemas, and client SDK generators — use `akua export`. No engine invocation, no customer inputs needed.

```sh
akua export --format=json-schema > inputs.schema.json     # for UI form rendering
akua export --format=openapi > inputs.openapi.json        # for API docs / codegen
```

For actual deployment rendering — use `akua render` with customer inputs (covered in §9).

The `export` output is pure, spec-compliant JSON Schema / OpenAPI 3.1 — no akua-specific extensions. Docstrings become `description`; decorators become `x-ui` metadata (which conforming renderers may use; others ignore). Consumers that speak these standards — including every JSON Schema tool in the ecosystem — work unchanged.

**No `x-user-input` or `x-input` markers.** Previous versions of akua layered custom extensions on JSON Schema to mark user-configurable fields and embed transforms. With KCL as the authoring substrate, both are redundant: the `Input` schema IS the customer-configurable contract by definition, and transforms live as KCL code in the package body. The exported JSON Schema is standards-pure; UI renderers in the broader ecosystem don't need to learn akua-specific vocabulary.

---

## 4. Body — engine calls + transforms

The body composes resources by calling engine functions and optionally transforming their output.

Engine calls return typed resource lists. Common patterns:

```python
# Helm with per-source input mapping (no chart fork needed to rename values)
_pg = helm.template(cnpg.Chart {
    values = cnpg.Values {
        cluster.name      = "${input.appName}-pg"
        cluster.instances = 3
        bootstrap.initdb.database = input.appName
    }
})

# Helm with postRenderer — per-resource transformation
_app = helm.template(webapp.Chart {
    values = webapp.Values { replicaCount = input.replicas }
    postRenderer = lambda r: dict -> dict {
        r.metadata.labels |= {"team": input.team}
        r
    }
})

# kro RGD — compile-time instantiation (offline, deterministic)
_glue = rgd.instantiate(platform_glue.RGD, {
    metadata.name: input.appName
    spec.domain:   input.routing.hostname
})

# Kustomize base
_addons = kustomize.build("./overlays/monitoring")
```

**Aggregating results:** concatenate with `[*a, *b, ...]`. Add extra resources declared in KCL:

```python
_servicemonitor = {
    apiVersion: "monitoring.coreos.com/v1"
    kind:       "ServiceMonitor"
    metadata.name: input.appName
    spec.selector.matchLabels.app: input.appName
}

resources = [*_pg, *_app, *_glue, *_addons, _servicemonitor]
```

**Schema-level validation via `check:` blocks** — this is KCL's role in the two-layer validation model (schema → Rego for cross-resource policy, see [policy-format.md](policy-format.md)):

```python
schema Deployment:
    spec: DeploymentSpec
    check:
        spec.replicas >= 1, "must have at least one replica"
        "app.kubernetes.io/name" in spec.template.metadata.labels,
            "every deployment must carry the app.kubernetes.io/name label"
```

KCL `check:` blocks evaluate at render time against each resource; failures surface as lint errors with line + field context.

---

## 5. The render output

`akua render --out ./deploy` writes every entry in `resources` as its
own YAML file in `./deploy/`. Filenames are deterministic
(`<NNN>-<kind>-<name>.yaml`), ordered by resource-list position.

```
deploy/
├── 000-configmap-hello.yaml
├── 001-service-hello.yaml
└── 002-deployment-hello.yaml
```

Raw manifests are akua's single render shape. Downstream systems that
want a different shape use one of:

- **In-body transformations** — a KCL function (present or future) that
  consumes resources and returns more K8s resources. `kro.rgd(...)`,
  `crossplane.composition(...)`, `kyverno.policy(...)` all fit this
  mould: they produce CRDs + composite resources that go into
  `resources` alongside everything else, and ship as plain YAML.
- **Future distribution verbs** — `akua publish --as helm-chart`
  wraps rendered manifests into a Helm chart at distribution time;
  `akua publish --as oci-bundle` signs and packages them. These are
  distribution concerns, not render concerns — the Package's `resources`
  are the input, not a pre-declared output list.

This keeps the Package shape trivially uniform: one `resources` list,
one render target. Authors reason about what exists; the CLI decides
how it ships.

---

## 6. Metadata

Optional top-level metadata that `akua` tooling surfaces in `inspect`, `diff`, and publishing:

```python
metadata = {
    name:        "payments-api"
    version:     "3.2.0"
    description: "Checkout + payments processing with managed Postgres"
    publisher:   "github.com/acme/payments"
    license:     "Apache-2.0"
    homepage:    "https://github.com/acme/payments"

    # Machine-readable keyword list for catalog discovery
    keywords: ["postgres", "webapp", "payments"]

    # Minimum akua version required to render this package
    requires: {
        akua:    ">=0.2.0"
        engines: { helm: ">=4.0", kcl: ">=0.12" }
    }
}
```

All fields optional; missing fields default to package-name-and-version derived from `akua.toml`. Publishing may require a `license` field depending on registry rules.

---

## 7. What's disallowed

Because determinism and the WASI sandbox are load-bearing:

- **No runtime I/O.** No `os.read`, `http.get`, `file.exists`, env-var lookups. KCL's sandbox enforces this.
- **No non-determinism.** No `random()`, no `now()`, no `uuid()`. Results depend only on `input` and imports.
- **No cluster reads at render time.** Use RGD output + kro for runtime late-binding; never a live query from KCL.
- **No `input` overwrite at runtime.** Inputs are provided once at render start and treated as immutable through the body.
- **No cross-source value imports.** Source A cannot reference Source B's output. Both derive from `input`. (Runtime cross-refs are the RGD case; see [policy-format.md](policy-format.md) for the broader framing.)

Violation of any of these is a compile error with a clear message.

---

## 8. Rendering model

`akua render`:

1. Parses `package.k` and type-checks the program.
2. Loads `input` from inputs file (YAML or KCL). Validates against the `Input` schema.
3. Resolves dependencies via `akua.toml` / `akua.lock`. Pulls and verifies signed artifacts.
4. Evaluates the KCL program. Every engine call happens here (in-process, sandboxed).
5. Collects the `resources` list — expanding any post-eval sentinels (`pkg.render`).
6. Writes each resource as its own YAML file under `--out`.
7. (Future) Writes `attestation.json` (SLSA v1 predicate) alongside the manifests.

Every step is deterministic: same inputs + same `akua.lock` + same `akua` version → byte-identical output.

---

## 9. Minimal example

The smallest possible Package:

```python
# package.k
import akua.helm
import charts.nginx as nginx

schema Input:
    hostname: str

input: Input = option("input") or Input {}

_nginx = helm.template(helm.Template {
    chart  = nginx.Chart
    values = nginx.Values {
        ingress.hostname = input.hostname
    }
})

resources = _nginx
```

See [examples/01-hello-webapp](../examples/01-hello-webapp/) for the fully runnable version.

---

## 10. Testing Packages

Packages ship with tests. The test runner is built into `akua test`; no separate framework required.

### Test file conventions

- `test_*.k` or `*_test.k` files anywhere under the package directory are discovered automatically.
- A test file is a KCL program that uses `assert` or `check:` blocks to express expectations against the package's render output or schema.

### Example — schema defaults

```python
# test_schema.k
import package as pkg

# Required fields are satisfied by sample inputs
_default_sample = pkg.Input {
    appName:  "test"
    hostname: "test.example.com"
}

# Assert default values
assert _default_sample.replicas == 3, "default replicas should be 3"
assert _default_sample.database.user == "app", "default db user should be 'app'"
```

### Example — render-output test

```python
# test_rendered.k
import akua.test as test
import package as pkg

# Render the package with specific inputs
_rendered = test.render(pkg, {
    appName:  "checkout"
    hostname: "checkout.example.com"
    replicas: 5
})

# Find the Deployment and assert its shape
_deployment = [r for r in _rendered if r.kind == "Deployment" and r.metadata.name == "checkout"][0]
assert _deployment.spec.replicas == 5, "replicas should flow through"
assert _deployment.spec.template.metadata.labels["team"] == "payments"
```

### Golden-output tests

When you want to pin the exact rendered YAML (catching unintended changes from dep bumps), add a `test.golden.yaml` alongside inputs:

```
tests/
├── basic/
│   ├── inputs.yaml
│   └── expected.golden.yaml
└── production/
    ├── inputs.yaml
    └── expected.golden.yaml
```

```sh
akua test --golden              # regenerate goldens if they drifted intentionally
akua test --golden=verify       # fail CI if goldens don't match (default in CI)
```

### Running

```sh
akua test                       # runs everything, including Rego tests
akua test --watch               # re-runs on file change (ideal for TDD)
akua test --coverage            # report per-schema / per-source coverage
akua test --filter=default      # only tests matching 'default'
```

Tests run via the embedded KCL engine (see [embedded-engines.md](embedded-engines.md)) — fast, sandboxed, deterministic.

### What to test

- Schema defaults and constraints — does `input.replicas = 0` correctly fail the `check:` block?
- Rendered-output shape — does the Deployment have the right labels, the right replicaCount?
- Policy compat — does rendering with a specific tier succeed? (Integration test; see [policy-format.md §9](policy-format.md#9-authoring-workflow))
- Upgrade compatibility — golden tests catch "dep bump accidentally changed the rendered manifest."

Packages without tests ship with a lint warning; platform teams can enforce a policy rule requiring tests for production-tier packages.

---

## 11. Relationship to other docs

- **[cli.md — `akua init` / `akua add` / `akua render` / `akua export` / `akua test` / `akua publish`](cli.md)** — the verbs that operate on packages. `render` runs the program; `export` converts the canonical form to a view.
- **[lockfile-format.md](lockfile-format.md)** — how `akua.toml` + `akua.lock` pin imports
- **[policy-format.md](policy-format.md)** — how Rego policies evaluate against rendered resources (separate concern from `check:` blocks)
- **[embedded-engines.md](embedded-engines.md)** — which engines run your tests
- **[examples/](../examples/)** — runnable Packages at increasing complexity
