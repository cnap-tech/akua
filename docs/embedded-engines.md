# Embedded engines

akua bundles every engine it dispatches to — KCL, OPA (Rego), Kyverno, CEL, Helm, kro, Kustomize, Regal — into the `akua` binary itself. No `$PATH` dependencies. No `helm` or `opa` or `kcl` required to be installed separately. One binary, everything works out of the box.

This doc covers the embedding strategy, per-engine status, and what it means for agents and CI. No shell-out escape hatch anywhere in the render pipeline — see [CLAUDE.md](../CLAUDE.md)'s "No shell-out, ever" invariant and [security-model.md](security-model.md) for the threat model.

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
hosted at runtime by the shared wasmtime Engine
(one Engine per process; one Store per invocation)
        │
        ▼
typed FFI: Rust host ↔ WASM guest
```

**Shared Engine, many Stores.** akua follows wasmtime's canonical pattern. One `engine_host_wasm::shared_engine()` singleton hosts:

- the `akua-render-worker` (per-render Store with the tenant's preopens + memory cap + epoch deadline);
- every engine plugin (helm, kustomize, future kro/CEL/kyverno) — each call gets its own Store on the same Engine.

Plugin callouts from sandboxed KCL cross the boundary once — through a single host-function import, `env::kcl_plugin_invoke_json_wasm`, that reads arguments from guest memory and dispatches to handlers registered against akua-core's plugin registry. The handler's engine Store runs, produces bytes, the bridge writes them back into the worker's linear memory. See [docs/security-model.md — one Engine, many Stores — with a plugin bridge](security-model.md#one-engine-many-stores--with-a-plugin-bridge) for the full picture + [docs/spikes/wasmtime-multi-engine.md](spikes/wasmtime-multi-engine.md) for the architecture decision.

Precompilation: each engine's `build.rs` calls `engine_host_wasm::precompile(...)` against `shared_config()` at akua build time, producing a `.cwasm` deserialized in ~microseconds on first use.

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
- An `akua bundle lock` manifest (forthcoming) will record the exact embedded engine versions for the workspace; `akua bundle verify` confirms a CI runner has the same akua version as the last known-good.

---

## Security posture

Every embedded engine runs inside the wasmtime WASI sandbox:

- No filesystem access beyond what akua-core explicitly mounts.
- No network access.
- No environment variable read-through.
- No syscall access.

This is stronger than shell-out — which would let binaries inherit the invoking shell's full privileges — and is why the shell-out fallback some other toolchains keep for convenience doesn't exist here. Agents operating akua in a sandbox can rely on the fact that no engine can escape to touch the rest of the system.

System integrations are a separate category from engines. `akua deploy` calls `kubectl` (or similar) because it genuinely does need cluster access; those verbs live outside the render pipeline, and the binaries they invoke are external tools the user already trusts on their path. The render pipeline itself — everything that transforms inputs into deploy-ready artifacts — never shells out.

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
