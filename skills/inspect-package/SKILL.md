---
name: inspect-package
description: Audit a published akua Package or Helm chart before deploying it — schema, sources, cosign signature, SLSA attestation chain, rendered output with sample inputs. Use when evaluating a third-party package, checking signatures, verifying supply-chain provenance, or inspecting a chart from ArtifactHub, GHCR, or any OCI registry.
license: Apache-2.0
---

# Inspect and audit a package before using it

Before consuming any package — first-party, community, or vendor — know what you're getting. This skill runs the full audit: schema, sources, signatures, attestation, rendered output.

## When to use

- Evaluating a package before adding it as a dependency
- Checking that a published package is signed by the expected publisher
- Verifying a chart's claimed behavior matches its actual rendered output
- Comparing a package version against another before upgrading
- Reviewing a third-party package in a browser tab with no local install

## Steps

### 1. Basic inspection

```sh
akua inspect oci://pkg.example.com/webapp:3.2 --json
```

Returns:

- The package's schema (required/optional inputs, types, defaults)
- Source list (Helm charts, RGDs, KCL modules consumed)
- Cosign signature + signer identity
- SLSA attestation predicate
- Declared output formats

Look at the `signer` field — it should match an identity you trust (a GitHub Actions workflow URL, a known team keyring, etc.). Unsigned packages are rejected by default unless `--insecure` is passed.

### 2. Show rendered output with sample inputs

```sh
akua inspect oci://pkg.example.com/webapp:3.2 \
  --inputs '{"appName":"demo","hostname":"demo.example.com"}' \
  --show=manifests
```

Renders the package with your inputs and prints the resulting Kubernetes YAML. This is exactly what would deploy. Audit it before committing.

### 3. Verify signatures directly

```sh
akua verify oci://pkg.example.com/webapp:3.2
```

Output includes:

- Signature valid / invalid
- Certificate identity (who signed)
- Certificate issuer (root of trust — Sigstore, custom CA)
- Attestation subjects and predicates

### 4. Browser audit (no install required)

If the package is on a public OCI registry, open it in the akua playground:

```
https://akua.dev/inspect?ref=oci://pkg.example.com/webapp:3.2
```

The playground renders in the browser using WASM — zero install, zero cluster, zero backend. Useful for sharing an audit link in a PR review or security ticket.

### 5. Diff against another version

```sh
akua diff oci://pkg.example.com/webapp:3.1 oci://pkg.example.com/webapp:3.2 --json
```

Shows structural changes: schema fields added/removed/type-changed, source version bumps, policy-compatibility verdict. Non-zero exit if any structural change present.

## Expected output

On `akua inspect --json`:

```json
{
  "ref": "oci://pkg.example.com/webapp:3.2",
  "digest": "sha256:…",
  "signed": true,
  "signer": "https://github.com/example/webapp/.github/workflows/release.yml",
  "schema": {
    "required": ["appName", "hostname"],
    "optional": ["replicas", "database"],
    "fields": 6
  },
  "sources": [
    {"kind": "helm", "chart": "cnpg-cluster", "version": "0.20.0"}
  ],
  "attestation": {
    "slsa_level": 3,
    "builder": "github.com/example/webapp/.github/workflows/release.yml"
  }
}
```

## Red flags to look for

- **`signed: false`** — unsigned. Do not deploy to any production environment.
- **`signer` unexpected** — a different GitHub workflow or identity than the project's known pipeline. Possible compromise or confusion attack.
- **`slsa_level` below 2** — no provenance recorded. Reduced trust.
- **Undocumented sources** — sources listed that the package's README doesn't mention. Could be legitimate; could be supply-chain injection.
- **`policy_compat: deny`** in a diff — upgrading this version would violate your current policy tier. Investigate before proceeding.

## Failure modes

- **`E_SIG_VERIFY_FAILED`** — signature does not verify. Either the artifact is tampered with, or the verification key is wrong. Do not proceed.
- **`E_FETCH_FAILED`** — cannot fetch the artifact. Check the OCI ref, check `akua whoami` for registry auth.
- **`E_ATTESTATION_MISSING`** — no SLSA predicate attached. Rendered output is unverifiable against a build pipeline. May be acceptable for community packages but is a red flag for production dependencies.

## Reference

- [cli.md — akua inspect](../../docs/cli.md#akua-inspect)
- [cli.md — akua verify](../../docs/cli.md#akua-verify)
- [cli.md — akua diff](../../docs/cli.md#akua-diff)
