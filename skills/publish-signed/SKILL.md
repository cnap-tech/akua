---
name: publish-signed
description: Publish an akua Package to an OCI registry with cosign signature and SLSA v1 attestation. Use when releasing a package version, cutting a new release in CI, shipping to a production registry, meeting supply-chain requirements like SLSA Level 3, or setting up a signed-by-default publishing pipeline.
license: Apache-2.0
compatibility: Requires a cosign-compatible signing key (keyless via Sigstore is supported; OIDC-linked GitHub Actions keyless works out of the box).
---

# Publish a signed + attested Package

Default akua publish behavior signs the package with cosign and generates a SLSA v1 provenance predicate. No extra flags needed. This skill walks through the path, the verification, and the CI integration.

## When to use

- Releasing a package version for a production reconciler to consume
- Cutting a first-party chart for the ecosystem (you're a project maintainer)
- Meeting SLSA Level 3 for compliance
- Bootstrapping a signed-by-default publishing pipeline

## Steps

### 1. Authenticate to the registry

Interactive:

```sh
akua login ghcr.io
```

CI (non-interactive, using a token):

```sh
akua login ghcr.io --token=$GITHUB_TOKEN
```

Keyless Sigstore (no key material to manage):

- Works out-of-the-box in GitHub Actions via OIDC
- Works locally with `cosign login` flow

### 2. Dry-run the publish

```sh
akua publish --to oci://ghcr.io/you/my-app --tag v1.0.0 --plan
```

Plan output: target ref, tag, digest (predicted), size, whether signing and attestation will apply, policy verdict.

### 3. Publish

```sh
akua publish --to oci://ghcr.io/you/my-app --tag v1.0.0
```

This:

- Builds the package (runs `akua render` internally against the declared default inputs; rejects if the package fails to render)
- Computes content-addressable digest
- Pushes to the target OCI registry
- Generates SLSA v1 predicate from build environment
- Signs both the package blob and the attestation with cosign
- Writes the attestation as an OCI referrer of the package

Output:

```json
{
  "package": "ghcr.io/you/my-app",
  "version": "1.0.0",
  "digest": "sha256:…",
  "signed": true,
  "attestation_digest": "sha256:…",
  "size_bytes": 1045832,
  "upload_duration_ms": 1823
}
```

### 4. Verify after publish

```sh
akua verify oci://ghcr.io/you/my-app:v1.0.0
```

Confirms: signature valid, signer identity matches expected, SLSA predicate present and valid, chain terminates at a trusted root.

Also works in the browser for public packages: `https://akua.dev/inspect?ref=oci://ghcr.io/you/my-app:v1.0.0`.

## CI workflow (GitHub Actions, keyless)

`.github/workflows/release.yml`:

```yaml
name: release

on:
  push:
    tags: ['v*']

permissions:
  contents: read
  packages: write
  id-token: write        # required for keyless cosign signing

jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install akua
        run: curl -fsSL https://akua.dev/install | sh

      - name: Publish
        run: |
          akua login ghcr.io --token=${{ secrets.GITHUB_TOKEN }}
          akua publish --to oci://ghcr.io/${{ github.repository }} --tag ${{ github.ref_name }}

      - name: Verify
        run: |
          akua verify oci://ghcr.io/${{ github.repository }}:${{ github.ref_name }}
```

The `id-token: write` permission is what enables keyless signing — GitHub Actions OIDC → Sigstore Fulcio → signing certificate embedded in the signature. No key material to manage or rotate.

## Agent-friendly invocation

An agent releasing a package should always:

1. Dry-run with `--plan --json`, check the policy verdict and the digest prediction
2. Use `--idempotency-key=<uuid>` so retry on network errors doesn't double-publish
3. Verify post-publish before considering the release complete

```sh
IDEMP=$(uuidgen)
akua publish --to oci://ghcr.io/you/my-app --tag v1.0.0 \
  --idempotency-key=$IDEMP --json | tee publish.json
akua verify oci://ghcr.io/you/my-app:v1.0.0 --json | tee verify.json
```

If `verify.json.signed` is not `true` or the signer identity doesn't match expected, the release is not complete; surface to a human.

## Policy-gated publish

Policy tiers can require:

- Keyless signing (no long-lived keys)
- SLSA Level 3 builder (specific GitHub Actions workflow pattern)
- Tag format (e.g., semver only)
- Approvers (two-person review before production publish)

```sh
akua publish ... --plan --policy=tier/production --json
```

Non-zero exit (code 3) if policy denies. Code 5 if needs approval; the output includes an approval URL agents/humans can follow.

## Failure modes

- **`E_SIGN_FAILED`** — cosign can't acquire a signing certificate. Check OIDC setup (GitHub Actions `id-token: write`) or `cosign login`.
- **`E_PUSH_FAILED`** — registry rejected the push. Auth (`akua whoami`), registry quota, or network.
- **`E_ATTESTATION_FAILED`** — SLSA predicate generation failed. Usually a missing build-environment field; error message specifies which.
- **Tag already exists** — `akua publish` refuses to overwrite by default. Use `--overwrite` only if you're intentionally republishing a fixed version (and update the policy to allow it).

## Reference

- [cli.md — akua publish](../../docs/cli.md#akua-publish)
- [cli.md — akua attest](../../docs/cli.md#akua-attest)
- [cli.md — akua verify](../../docs/cli.md#akua-verify)
- [SLSA Level 3 requirements](https://slsa.dev/spec/v1.0/levels#build-l3)
