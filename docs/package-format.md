# Package format

The canonical shape of an akua Package. A Package is a reusable definition authored in **KCL** and published as a signed OCI artifact. One Package produces many Apps across environments and customers.

This document specifies what a `package.k` file may contain. Companion references: [krm-vocabulary.md](krm-vocabulary.md) for the KRM kinds, [lockfile-format.md](lockfile-format.md) for `akua.mod` / `akua.sum`, [policy-format.md](policy-format.md) for Rego.

---

## 1. Anatomy

Every Package is one KCL program with four typed regions:

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

input: Input

# (3) body — source engine calls + transforms + aggregation
_pg  = helm.template(cnpg.Chart { values = cnpg.Values { ... } })
_app = helm.template(webapp.Chart { values = webapp.Values { ... } })

resources = [*_pg, *_app]

# (4) outputs — what formats akua emits
outputs = [
    { kind: "RawManifests", target: "./" }
]
```

The four regions have strict rules. Everything else is disallowed or by convention.

---

## 2. Imports

An import brings one of three things into scope:

| import form | purpose | pinned by |
|---|---|---|
| `import akua.<engine>` | a source-engine callable (`helm`, `rgd`, `kustomize`, `oci`) | the akua CLI version |
| `import charts.<name>` | a typed source package previously added via `akua add` | `akua.mod` |
| `import <local/path>` | a local KCL module within this package | the filesystem |

Imports are resolved at build time against `akua.mod` (declared deps) and verified against `akua.sum` (digest + signature). Failed verification is a compile error. A missing pin is a compile error.

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

- Must be named `Input` (lowercase `input: Input` bound below).
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

## 5. Outputs — what akua emits

The `outputs` array declares target format(s). Each item has optional `name` for per-source routing (see §6).

```python
outputs = [
    # Default — raw manifests committed to git
    {
        kind:   "RawManifests"
        target: "./"
    }
]
```

Supported `kind` values and their shapes:

| kind | artifact | when to use |
|---|---|---|
| `RawManifests` | YAML files under `target/` | default for Compiled GitOps; ArgoCD/Flux/kubectl consume |
| `HelmChart` | `Chart.yaml` + templates (or `.tgz` at publish time) | customer needs Helm release lifecycle |
| `ResourceGraphDefinition` | a kro-compatible RGD | late-binding at runtime via kro controller |
| `Crossplane` | XR + Composition | multi-cloud infra compositions |
| `OCIBundle` | multi-layer OCI artifact | signed distribution; future-format-ready |
| `WASMRenderer` | self-hosting artifact containing the renderer | v2 / Gen-4 / edge / browser |

Each output kind may accept additional fields specific to that format:

```python
outputs = [
    {
        kind:   "RawManifests"
        target: "./deploy"
    },
    {
        kind:      "HelmChart"
        target:    "oci://pkg.example.com/my-app"
        chartName: input.appName
        appVersion: "1.0.0"
    },
    {
        kind:   "ResourceGraphDefinition"
        target: "./rgd"
        name:   "platform-app"
    }
]
```

---

## 6. Per-source output routing

Advanced: a package with mixed runtime requirements can route different sources to different outputs. Most packages have one output and don't use this.

```python
outputs = [
    { name: "static",  kind: "RawManifests", target: "./deploy" },
    { name: "runtime", kind: "ResourceGraphDefinition", target: "./deploy/rgd" }
]

_pg  = helm.template(cnpg.Chart { ... }, output = "static")
_app = helm.template(webapp.Chart { ... }, output = "static")
_glue = rgd.instantiate(glue.RGD, { ... }, output = "runtime")   # needs runtime late-binding
```

An omitted `output = ...` defaults to every unnamed output. Two reconcilers can cooperate on the same deploy because they create different resources with different owner references.

---

## 7. Metadata

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

All fields optional; missing fields default to package-name-and-version derived from `akua.mod`. Publishing may require a `license` field depending on registry rules.

---

## 8. What's disallowed

Because determinism and the WASI sandbox are load-bearing:

- **No runtime I/O.** No `os.read`, `http.get`, `file.exists`, env-var lookups. KCL's sandbox enforces this.
- **No non-determinism.** No `random()`, no `now()`, no `uuid()`. Results depend only on `input` and imports.
- **No cluster reads at render time.** Use RGD output + kro for runtime late-binding; never a live query from KCL.
- **No `input` overwrite at runtime.** Inputs are provided once at render start and treated as immutable through the body.
- **No cross-source value imports.** Source A cannot reference Source B's output. Both derive from `input`. (Runtime cross-refs are the RGD case; see [policy-format.md](policy-format.md) for the broader framing.)

Violation of any of these is a compile error with a clear message.

---

## 9. Rendering model

`akua render`:

1. Parses `package.k` and type-checks the program.
2. Loads `input` from inputs file (YAML or KCL). Validates against the `Input` schema.
3. Resolves dependencies via `akua.mod` / `akua.sum`. Pulls and verifies signed artifacts.
4. Evaluates the KCL program. Every engine call happens here (in-process, sandboxed).
5. Collects the `resources` list and partitions by output routing.
6. Emits each output according to its `kind`.
7. Writes `attestation.json` (SLSA v1 predicate) to the primary output directory.

Every step is deterministic: same inputs + same `akua.sum` + same `akua` version → byte-identical output.

---

## 10. Minimal example

The smallest possible Package:

```python
# package.k
import akua.helm
import charts.nginx as nginx

schema Input:
    hostname: str

input: Input

_nginx = helm.template(nginx.Chart {
    values = nginx.Values {
        ingress.hostname = input.hostname
    }
})

resources = _nginx

outputs = [
    { kind: "RawManifests", target: "./" }
]
```

See [examples/01-hello-webapp](examples/01-hello-webapp/) for the fully runnable version.

---

## 11. Testing Packages

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

## 12. Relationship to other docs

- **[cli.md — `akua init` / `akua add` / `akua render` / `akua test` / `akua publish`](cli.md)** — the verbs that operate on packages
- **[lockfile-format.md](lockfile-format.md)** — how `akua.mod` + `akua.sum` pin imports
- **[policy-format.md](policy-format.md)** — how Rego policies evaluate against rendered resources (separate concern from `check:` blocks)
- **[krm-vocabulary.md](krm-vocabulary.md)** — how App, Environment, Policy KRMs interact with Packages
- **[embedded-engines.md](embedded-engines.md)** — which engines run your tests
- **[examples/](examples/)** — runnable Packages at increasing complexity
