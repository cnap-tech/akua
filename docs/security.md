# Security model

akua's core security property is simple: **chart rendering is sandboxed, supply chain is pinned, and provenance is signed**. This document explains what that means, why it matters, and where the trust boundary sits.

---

## The problem akua solves: untrusted Helm rendering

ArgoCD's repo-server executes `helm template` to render charts before committing them to the cluster. That rendering happens in a shared process with access to the host filesystem, adjacent Git repositories, and any secrets mounted into the repo-server pod. Several CVEs have exploited this:

| CVE | Attack | Impact |
|---|---|---|
| **CVE-2022-24348** | Path traversal via specially crafted Helm chart | Read files outside the repo root — SOPS keys, adjacent secrets, API credentials cached on the repo-server |
| **CVE-2024-29893** | Malicious Helm registry pushes continuous data | OOM crash in repo-server during chart fetch |
| Supply chain (ongoing) | Typosquatting on Artifact Hub; infected chart maintainer accounts | Arbitrary Kubernetes manifests rendered and committed to cluster |

The root cause in all cases is the same: **untrusted rendering code runs with the same privileges as the system that holds your secrets**.

---

## How akua eliminates these vectors

### 1. Rendering inside a WASM sandbox

akua renders Helm charts using a Go→wasip1 WASM module hosted by `wasmtime` with zero I/O imports:

```
Helm chart (in-memory)
        │
        ▼
wasmtime guest (helm-engine-wasm)
  - no filesystem mount
  - no network socket
  - no env read-through
  - no host syscalls
        │
        ▼
rendered manifests (in-memory bytes)
```

A malicious chart has no path traversal target because **there is no filesystem to traverse**. CVE-2022-24348-class attacks require the renderer to be able to stat or open paths on the host; inside wasmtime with no mounts, there are none.

The OOM vector (CVE-2024-29893) is bounded by wasmtime's per-guest memory limit. A chart that attempts to allocate unbounded memory hits the WASM linear memory ceiling and returns an error rather than crashing the host process.

This is not a mitigation bolted onto `helm template` — it is a structural consequence of the rendering architecture. The Helm engine never runs as a native binary; it runs as a WASM guest with the same permissions as a calculator.

### 2. Digest-pinned lockfile — no silent drift

ArgoCD, by default, fetches Helm charts by tag at sync time. Tags are mutable: a supply chain attacker who controls the chart registry can replace `v1.2.3` with malicious content after your ArgoCD config points to it.

akua uses a content-addressed lockfile (`akua.lock`) that pins every chart dependency to a SHA-256 digest at `akua publish` time:

```toml
# akua.lock (committed to git)
[[sources]]
name    = "postgres"
ref     = "oci://registry.example.com/postgres:15.3"
digest  = "sha256:a3b4c5d6..."
```

At render time, akua verifies the digest before invoking the Helm engine. If the registry has been tampered with — tag moved, image replaced — the digest check fails and rendering aborts. The chart content that reaches the WASM engine is exactly the content that was audited at publish time.

### 3. Cosign signatures + SLSA v1 attestations

Every `akua publish` emits:

- A **cosign signature** over the package OCI digest (keyless via Sigstore, or key-based).
- A **SLSA v1 Build predicate** recording builder identity, source commit, and the set of input digests.

Consumers verify by default on `akua pull`:

```sh
akua pull oci://registry.example.com/my-app:1.0.0
# → verifies cosign signature
# → verifies SLSA predicate digest chain
# → refuses to proceed if either fails
```

This gives you a cryptographic chain from source commit to deployed manifests. A supply chain attacker who can push to the registry but not forge signatures cannot introduce unsigned artifacts into the render pipeline.

### 4. No network access during render

The ArgoCD repo-server fetches chart dependencies at render time. This opens SSRF attack surfaces: a chart can declare a `repository:` pointing at an internal service, and the fetch happens with repo-server's network privileges.

akua separates **resolution** (happens at `akua publish` time, produces `akua.lock`) from **rendering** (happens offline, using only the already-fetched + digest-verified content). The Helm WASM engine receives charts as in-memory bytes with no ability to initiate network calls. There is no SSRF surface during rendering.

---

## KCL programs: same sandbox model

User-authored KCL programs are evaluated in a `wasm32-wasip1` guest with bounded WASI capabilities (see [security-model.md — Execution model](security-model.md#execution-model)). A malicious KCL program cannot:

- Read the host filesystem.
- Open network connections.
- Access environment variables.
- Escape to the host process.

This applies whether the evaluator is the CLI binary or the `@akua-dev/sdk` library.

---

## What akua does NOT protect against

| Scope | Who is responsible |
|---|---|
| Cluster-side reconciler (ArgoCD / Flux / kro) permissions | Cluster operator — apply least-privilege RBAC to the reconciler service account |
| Kubernetes RBAC for deployed workloads | Platform team — the manifests akua renders may request cluster-admin; your admission policy should gate that |
| Secrets management at deploy time | Your secret store (Vault, infisical, SOPS) — akua never reads or renders secrets into manifests |
| Compromised akua binary itself | You — verify the akua binary's own cosign signature before use: `cosign verify-blob ...` |
| Registry access control | Your registry operator — akua verifies what it pulls, but cannot prevent you from granting write access to untrusted parties |

---

## Comparison: ArgoCD repo-server vs akua

| property | ArgoCD repo-server (`helm template`) | akua (Helm WASM engine) |
|---|---|---|
| Renderer isolation | Shared process; full host filesystem access | wasmtime WASM guest; zero filesystem |
| Network access during render | Yes (chart dep fetch) | No (fetch separated; render is offline) |
| Path traversal risk | Yes (CVE-2022-24348) | Structurally eliminated |
| OOM from malicious registry | Yes (CVE-2024-29893) | Bounded by WASM memory limit |
| Supply chain pinning | Optional (tag-based by default) | Mandatory (`akua.lock` digest pinning) |
| Provenance | None by default | cosign signature + SLSA v1 predicate |
| Signature verification on pull | Not built in | Default |

---

## See also

- [Security model](security-model.md) — implementation-level threat model, sandbox spec, plugin bridge boundary
- [Embedded engines](embedded-engines.md) — WASM sandbox details and security posture section
- [Lockfile format](lockfile-format.md) — digest pinning and `akua.lock` schema
- [CLI contract §1.3 determinism](cli-contract.md#13-determinism) — why the same inputs always produce the same outputs
- [Package format](package-format.md) — how sources are declared (upfront, in `akua.toml`, not at render time)
