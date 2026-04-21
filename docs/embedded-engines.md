# Embedded engines

akua bundles every engine it dispatches to — KCL, OPA (Rego), Kyverno, CEL, Helm, kro, Kustomize, Regal — into the `akua` binary itself. No `$PATH` dependencies. No `helm` or `opa` or `kcl` required to be installed separately. One binary, everything works out of the box.

This doc covers the embedding strategy, per-engine status, the shell-out fallback, and what it means for agents and CI.

---

## Why embed

Three reasons, same as the helm-engine-wasm decision:

1. **Single binary UX.** `curl -fsSL https://akua.dev/install | sh` gives you everything. No "now install helm 4.1.4" followed by "now install opa 0.60" followed by "now install KCL 0.12."
2. **Version determinism.** akua ships with a known-good engine version. No "works on my machine" where my `opa` is 0.55 and yours is 0.62 and we get different verdicts.
3. **Air-gap friendly.** Environments where customers can't install arbitrary binaries (FedRAMP, certain enterprise networks) still work because akua is self-contained.

Plus the agent case: agents can't install binaries. If `akua test` needs `opa` and there's no `opa` in the sandbox, the agent is stuck. Embedded means the agent gets the full toolkit from one install.

---

## Embedding strategy

Every engine reaches akua through the same architecture as `helm-engine-wasm`:

```
engine source (Go / Rust / C++)
        │
        ▼
compiled to wasip1 WASM module
        │
        ▼
shipped inside the akua binary via include_bytes!
        │
        ▼
hosted at runtime by wasmtime (embedded in akua-core)
        │
        ▼
typed FFI: Rust host ↔ WASM guest
```

For Rust engines (KCL via `kclvm-rs`), we link directly in-process — no WASM wrapping needed.

Precompilation caches are written to `$XDG_CACHE_HOME/akua/modules/` on first use; subsequent invocations deserialize the precompiled module in single-digit milliseconds.

---

## Engine inventory

| engine | source language | embedding method | v0 status |
|---|---|---|---|
| **KCL** (package authoring) | Rust (`kclvm-rs`) | direct link | shipped |
| **Helm v4 template engine** | Go → wasip1 | wasmtime-hosted | shipped (forked to strip client-go; ~20 MB WASM) |
| **OPA** (Rego) | Go → wasip1 or OPA-native WASM | wasmtime-hosted | v0.2 |
| **Regal** (Rego linter) | Go → wasip1 | wasmtime-hosted | v0.2 |
| **Kyverno-to-Rego converter** | Go → wasip1 | wasmtime-hosted; runs at `akua add` time | v0.3 |
| **CEL** (`cel-go`) | Go → wasip1 | wasmtime-hosted | v0.3 |
| **kustomize** | Go → wasip1 | wasmtime-hosted | v0.3 |
| **kro RGD instantiator** | Go → wasip1 (offline path) | wasmtime-hosted | v0.2 |

All compiled to wasip1 where practical. When upstream projects ship optimized WASM artifacts (OPA has `opa build -t wasm`), we use theirs; otherwise we compile from source in our CI and ship the artifact with akua.

Binary size impact per engine: KCL ~8 MB, Helm (stripped fork) ~20 MB, OPA (with Regal) ~15 MB, Kyverno converter ~18 MB, CEL ~5 MB, Kustomize ~12 MB, kro instantiator ~6 MB. Total overhead versus a bare akua: ~85 MB. We consider this acceptable for "everything just works" — same order of magnitude as Bun (~45 MB) or Deno (~110 MB).

---

## Shell-out fallback

Not all scenarios need embedded. Sometimes an air-gapped team has a specific OPA version they must use; sometimes a customer wants to test against their live Kyverno installation.

Every engine-driven verb accepts `--engine=<auto|embedded|shell>`:

| mode | behavior |
|---|---|
| `auto` (default) | use embedded; if a newer version is detected on `$PATH`, use that instead |
| `embedded` | strictly use akua's bundled engine; never fall back |
| `shell` | strictly shell out to the binary on `$PATH`; fail if not found |

```sh
akua test                       # auto — uses embedded OPA
akua test --engine=shell        # shells to opa on $PATH
akua test --engine=embedded     # forces embedded (matches CI-reproducible builds)
```

Env var equivalent: `AKUA_ENGINE=embedded|shell|auto`.

`akua version --json` reports which engines are embedded vs shelled per verb:

```json
{
  "akua": "0.2.1",
  "engines": {
    "kcl":      { "version": "0.12.0", "mode": "embedded" },
    "helm":     { "version": "4.1.4",  "mode": "embedded" },
    "opa":      { "version": "0.60.0", "mode": "shell",  "path": "/opt/homebrew/bin/opa" },
    "regal":    { "version": "0.22.0", "mode": "embedded" },
    "kustomize":{ "version": "5.3.0",  "mode": "embedded" }
  }
}
```

---

## Per-verb engine routing

Each verb that invokes engines documents which ones. From [cli.md](cli.md):

| verb | engines used |
|---|---|
| `akua init` | KCL (scaffold) |
| `akua add` | (fetch/convert) Kyverno-to-Rego converter, KCL schema generator |
| `akua render` | KCL + Helm + kro offline instantiator + Kustomize + output emitters |
| `akua lint` | KCL + Regal |
| `akua fmt` | KCL + opa fmt |
| `akua check` | KCL + OPA (parse-only) |
| `akua test` | KCL + OPA |
| `akua trace` | OPA (`--explain`) |
| `akua bench` | OPA partial evaluation, KCL interpreter timing |
| `akua policy check` | OPA + CEL (via Rego runtime) |
| `akua repl` | KCL + OPA |
| `akua eval` | KCL or OPA per `--lang` |
| `akua attest` | (no engines; just signing + SLSA predicate generation) |
| `akua diff` | KCL + OPA (for policy compat diff) |

---

## Determinism guarantees

- Embedded engines are version-pinned to the akua release. Two runs of `akua render` at the same akua version produce byte-identical output (the [CLI contract §1.3](cli-contract.md#13-determinism)).
- Shell-out engines break this guarantee unless the user has explicitly pinned their `$PATH` binary. CI should use `--engine=embedded` for reproducible pipelines.
- An `akua bundle lock` manifest (forthcoming) will record the exact embedded engine versions for the workspace; `akua bundle verify` confirms a CI runner has the same akua version as the last known-good.

---

## Security posture

Every embedded engine runs inside the wasmtime WASI sandbox:

- No filesystem access beyond what akua-core explicitly mounts.
- No network access.
- No environment variable read-through.
- No syscall access.

This is stronger than shell-out, where binaries inherit the invoking shell's full privileges. Agents operating akua in a sandbox can rely on the fact that no engine can escape to touch the rest of the system.

The one exception: `akua deploy` shells to `kubectl` or similar because those verbs genuinely do need cluster access. Engines (things that transform inputs to outputs) are sandboxed; system integrations (things that touch cluster state) use standard binaries.

### KCL sandboxing — CLI vs SDK

In the CLI binary, KCL is linked directly (`kclvm-rs` native Rust crate) — fast, full feature set. In the **SDK** (`@akua/sdk`), the KCL compiler is compiled to `wasm32-wasi` and hosted by wasmtime with zero I/O imports. This means user-supplied KCL programs evaluated inside an SDK host cannot escape to the filesystem or network, even if they contain malicious code. The WASM guest has no import for file I/O, no network, no env reads.

This is the same embedding model as `helm-engine-wasm`. The performance overhead is ~2× for typical workloads (verified: ~31 ms at 500 resources vs ~15 ms native; within acceptable bounds for the workflow use-case). AOT precompilation cuts module init to ~8 ms. See [docs/performance.md](./performance.md).

---

## What's NOT embedded

- **`kubectl`** — used only by `akua deploy --to=kubectl`. Too specific to a user's cluster context; we rely on the system version.
- **`git`** — used for `akua publish` and workspace operations. Extremely stable and universally available.
- **`cosign`** — used for signing. We embed the verification path (cryptographic primitives are in akua-core) but use `cosign` CLI for signing operations that need hardware keys.
- **`docker` / `podman`** — used only if a user opts into a Dockerfile-based build. Rare for akua workflows.

---

## Performance notes

- Cold-start overhead for a wasmtime-hosted engine: ~5-30 ms per engine, once per `akua` invocation. With precompile cache: ~2-5 ms.
- `akua dev` keeps engines warm for the session. Subsequent renders skip cold-start entirely.
- Benchmarks at [docs/bench/](bench/) (forthcoming) show akua's embedded OPA within 5% of native `opa eval` for realistic policy workloads.

---

## For agents

Agents get the full engine toolkit from one install with zero PATH management. When writing skills that invoke `akua test`, `akua fmt`, `akua bench`, they never need to check `which opa`. Skills remain portable across fresh sandboxes, CI runners, and developer laptops without setup instructions beyond `curl -fsSL https://akua.dev/install | sh`.

---

## Relationship to other docs

- **[cli.md](cli.md)** — the verbs that use these engines
- **[package-format.md](package-format.md)** — KCL (the primary host)
- **[policy-format.md](policy-format.md)** — OPA / Rego / Kyverno / CEL (the policy host + pluggable guests)
- **[cli-contract.md §1.3 determinism](cli-contract.md#13-determinism)** — why embedding matters for reproducibility
