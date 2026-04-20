# `akua.mod` and `akua.sum`

akua's package manager is modeled on Go's: a human-edited manifest of declared intent, and a machine-maintained ledger of resolved digests + signatures.

- **`akua.mod`** — what you asked for. Declared deps, version constraints, workspace members. Human-edited.
- **`akua.sum`** — what you got. Digest and cosign signature per resolved artifact. Machine-maintained, never hand-edited.

Both files are TOML. Both are checked into git. Both are required.

---

## Why two files

Clear separation of concerns:

| | intent | evidence |
|---|---|---|
| file | `akua.mod` | `akua.sum` |
| edited by | human | `akua add` / `akua pull` |
| shape | small, stable | large, churn on every upgrade |
| review focus | "do we want this dep?" | "is this the expected digest?" |

A PR that modifies `akua.sum` but not `akua.mod` is automatically suspicious (someone changed what they got without changing what they asked for). CI can lint for this.

Merged-lockfile alternatives (npm's `package-lock.json`, Cargo's `Cargo.lock`) bundle both concerns into one file; we prefer Go's split for review hygiene.

---

## `akua.mod`

### Top-level structure

```toml
[package]
name    = "my-app"
version = "0.1.0"
edition = "akua.dev/v1alpha1"                 # akua schema compat marker

# (Optional) workspace members — for monorepos with many akua packages
[workspace]
members = ["./", "./apps/*"]

# Dependencies — every import in KCL / Rego must be declared here
[dependencies]
# key = { source_type = "<ref>", version = "..." }
```

### Dependency forms

| form | example | use when |
|---|---|---|
| OCI | `{ oci = "oci://ghcr.io/.../foo", version = "1.2.3" }` | published signed artifact (most common) |
| Git | `{ git = "https://github.com/foo/bar", tag = "v1.2.3" }` | non-OCI-distributed sources |
| Path | `{ path = "../shared" }` | workspace-local, dev-only |
| Replace | `{ oci = "...", replace = { path = "../fork" } }` | local-fork override for debugging |

### Example

```toml
[package]
name    = "payments-api"
version = "3.2.0"
edition = "akua.dev/v1alpha1"

[dependencies]
# KCL sources
k8s       = { oci = "oci://ghcr.io/kcl-lang/k8s",              version = "1.31.2" }
cnpg      = { oci = "oci://ghcr.io/cloudnative-pg/charts/cluster", version = "0.20.0" }
webapp    = { oci = "oci://ghcr.io/acme/charts/webapp",        version = "2.1.0" }

# Rego policies (compile-resolved as data. imports — see policy-format.md)
tier-prod = { oci = "oci://policies.akua.dev/tier/production", version = "1.2.0" }
kyv-sec   = { oci = "oci://policies.akua.dev/kyverno/security", version = "2.0.0" }

# Local fork for debugging (points at sibling directory)
our-glue  = { oci = "oci://pkg.acme.internal/glue", version = "0.3.0",
              replace = { path = "../glue-fork" } }
```

### Version resolution

- **Exact pin preferred.** `version = "1.2.3"` means that version, nothing else. No implicit semver-range resolution.
- **Semver range allowed** (`version = "^1.2.0"`) for dependencies where minor updates are trusted. akua's resolver picks the highest matching version satisfying all constraints across the graph.
- **Conflicts error out.** If two dependencies pin different versions of a shared transitive, the resolver fails with a clear message. Use `replace` to force a single version.

### Fields

| field | required | notes |
|---|---|---|
| `[package].name` | yes | a valid KCL package identifier |
| `[package].version` | yes | semver |
| `[package].edition` | yes | `akua.dev/v1alpha1` for v0 compatibility |
| `[workspace].members` | no | glob patterns; enables monorepo |
| `[dependencies]` | yes (can be empty) | see dependency forms above |

---

## `akua.sum`

### Format

A plain-text line-per-dependency-per-version file. Each line is:

```
<name> <version> <source-ref> <digest> <signature>
```

Columns are whitespace-separated. Example:

```
k8s        1.31.2  oci://ghcr.io/kcl-lang/k8s                        sha256:a1b2c3…  cosign:sigstore:…
cnpg       0.20.0  oci://ghcr.io/cloudnative-pg/charts/cluster      sha256:d4e5f6…  cosign:sigstore:…
tier-prod  1.2.0   oci://policies.akua.dev/tier/production          sha256:g7h8i9…  cosign:sigstore:…
kyv-sec    2.0.0   oci://policies.akua.dev/kyverno/security         sha256:j0k1l2…  cosign:sigstore:…
```

### Rules

- **Digest is always content-addressable.** `sha256:` for OCI, SHA-256 over tarball for git.
- **Signature is cosign-verifiable.** Keyless (`cosign:sigstore:…`) or keyed (`cosign:key:…`). Missing signatures allowed only when `[package].strictSigning = false` in `akua.mod` — CI should reject unsigned unless this is explicitly opted out.
- **One line per resolved version.** Transitive dependencies appear with their actual resolved version, not the range declared upstream.
- **Alphabetical sort.** Stable diff even across unrelated PRs.
- **Trailing newline.** POSIX file discipline.

### What `akua.sum` does NOT contain

- Source code (not a vendor directory)
- Mutable metadata (timestamps, author info)
- Version ranges (those live in `akua.mod`)
- Comments (it's machine-generated; put explanations in `akua.mod`)

---

## Resolution workflow

### `akua add <kind> <ref> --version=<v>`

1. Reads current `akua.mod`
2. Adds the new entry to `[dependencies]`
3. Fetches the artifact; computes digest
4. Verifies cosign signature
5. Appends a line to `akua.sum`
6. If the new dep transitively pulls others, repeats for each

Result: `akua.mod` and `akua.sum` both updated in one atomic operation.

### `akua verify` (CI gate)

1. Reads `akua.mod` and `akua.sum`
2. Resolves every dep from `akua.mod`
3. Compares expected (mod) vs locked (sum) digest + signature
4. Exits 0 if everything matches; non-zero otherwise

Run in CI on every PR to catch lockfile tampering.

### `akua update [dep]`

Updates to the highest allowed version per `akua.mod` constraints; rewrites the relevant `akua.sum` line(s). Leaves other deps untouched unless their constraints also match a new version.

### `akua vendor` (optional)

Writes full dependency tree to `./vendor/` for air-gapped builds. Content matches `akua.sum` digests. Not needed for online builds.

---

## Workspaces

For monorepos with multiple akua packages:

```
platform/
├── akua.mod                     # workspace root
├── akua.sum
├── apps/
│   ├── api/
│   │   └── package.k            # member package
│   ├── worker/
│   │   └── package.k            # member package
│   └── dashboard/
│       └── package.k            # member package
└── policies/
    └── org-baseline/
        └── policy.rego          # member policy module
```

Workspace root `akua.mod`:

```toml
[workspace]
members = ["./apps/*", "./policies/*"]

[dependencies]
# Shared deps used by all members
k8s = { oci = "oci://ghcr.io/kcl-lang/k8s", version = "1.31.2" }
```

Members inherit workspace dependencies; they may override in a member-local `akua.mod` (minimal). Cross-member imports work as `path`-type deps.

---

## Compatibility with kpm

Akua's `akua.mod` is not the same as KCL's `kcl.mod`. We don't try to be. akua packages can contain a `kcl.mod` in their source tree for pure-KCL consumers who want to use upstream `kcl run` against the package directly; akua's resolver honors either file when it's unambiguous.

See [the broader architecture note](https://github.com/cnap-tech/cortex/blob/docs/cnap-masterplan/workspaces/robin/akua-masterplan.md) for the "akua is the outer package manager; kpm is the inner KCL-layer tool" framing.

---

## Example: a real workspace

```
./
├── akua.mod
├── akua.sum
├── apps/
│   ├── api/
│   │   └── package.k
│   └── worker/
│       └── package.k
├── policies/
│   └── production.rego
└── environments/
    ├── dev/inputs.yaml
    ├── staging/inputs.yaml
    └── production/inputs.yaml
```

`akua.mod`:

```toml
[package]
name    = "acme-platform"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[workspace]
members = ["./apps/*"]

[dependencies]
k8s       = { oci = "oci://ghcr.io/kcl-lang/k8s",              version = "1.31.2" }
cnpg      = { oci = "oci://ghcr.io/cloudnative-pg/charts/cluster", version = "0.20.0" }
webapp    = { oci = "oci://ghcr.io/acme/charts/webapp",        version = "2.1.0" }
tier-prod = { oci = "oci://policies.akua.dev/tier/production", version = "1.2.0" }
kyv-sec   = { oci = "oci://policies.akua.dev/kyverno/security", version = "2.0.0" }
```

`akua.sum` (after resolution):

```
cnpg       0.20.0   oci://ghcr.io/cloudnative-pg/charts/cluster       sha256:d4e5f6…  cosign:sigstore:…
k8s        1.31.2   oci://ghcr.io/kcl-lang/k8s                        sha256:a1b2c3…  cosign:sigstore:…
kyv-sec    2.0.0    oci://policies.akua.dev/kyverno/security          sha256:j0k1l2…  cosign:sigstore:…
tier-prod  1.2.0    oci://policies.akua.dev/tier/production           sha256:g7h8i9…  cosign:sigstore:…
webapp     2.1.0    oci://ghcr.io/acme/charts/webapp                   sha256:m3n4o5…  cosign:sigstore:…
```

CI runs `akua verify` on every PR; any digest mismatch or missing signature fails the build.
