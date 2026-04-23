# Security model

akua is a **sandboxed-by-default** render substrate. Every render runs inside a wasmtime WASI sandbox with memory / CPU / wall-clock caps and capability-model filesystem preopens. The invariant lives in [CLAUDE.md](../CLAUDE.md); this document records what that actually means, what's guaranteed, and what's not.

---

## Threat model

**Who is the adversary?** The Package itself — author of the KCL program + the charts, overlays, policies it depends on.

**Why?** akua is designed to run in shared multi-tenant environments (hosted build services, CI pipelines accepting PR-submitted Packages, in-browser dev loops loading third-party examples). In all three, the Package is untrusted by definition.

**What must we prevent?**

1. Reading files outside the Package directory and its explicit dep scope
2. Writing files outside the designated output directory
3. Making network requests
4. Spawning subprocesses
5. Exhausting host memory
6. Exhausting host CPU (runaway loops, pathological schema evaluation)
7. Exceeding a wall-clock deadline
8. Escaping the sandbox to compromise the host process

**What are we NOT trying to prevent?**

- **Declarative mischief** in the Package's rendered output. akua produces whatever YAML the author writes. Policy evaluation ([docs/policy-format.md](policy-format.md)) is the layer that catches "this Package declares a root-privileged Deployment." The renderer's job is faithful execution of the Package, not moral judgment.
- **Side-channel leaks** (timing, memory-access patterns). wasmtime provides strong isolation but not constant-time; a sophisticated adversary could extract bits via timing. Not our bar.
- **Bugs in the dep supply chain beyond what the lockfile catches.** `akua.lock` pins OCI deps by sha256 of the chart blob; a drift between what the lockfile recorded and what the registry now serves is rejected (`LockDigestMismatch`). But if the *initial* `akua add` pulled from a compromised registry that served a malicious chart and recorded its digest, every subsequent render faithfully reproduces it. Phase 6 (cosign verification + SLSA attestation walk) closes this gap.

---

## Execution model

**The render path runs in a wasmtime WASI sandbox.** Concretely:

- **`akua-render-worker`** — akua's render pipeline compiled to `wasm32-wasip1`, AOT-compiled to `.cwasm` at akua's build time.
- **wasmtime host** — akua's native binary loads the `.cwasm` once per process and instantiates per-render via `InstanceAllocationStrategy::pooling(...)` (microsecond instantiation after first load).
- **Per-render `Store`** — fresh `WasiCtx` with the tenant's preopens, fresh `StoreLimits` with hard memory cap, fresh fuel + epoch deadlines. Dropped on render completion.

## What's guaranteed

| Threat | Defense |
|---|---|
| Read files outside scope | **Preopened dirs only.** WASI is a capability model: `WasiCtxBuilder::preopened_dir(host_path, guest_path, DirPerms::READ, FilePerms::READ)` hands exactly one directory to the guest. `/etc`, `/proc`, `$HOME`, `/var/run/secrets` — all unreachable because they're not mounted. There is no ambient filesystem; nothing to escape to. |
| Write files outside scope | Same mechanism. Output dir preopened with `DirPerms::MUTATE + FilePerms::WRITE`; nothing else writable. |
| Network | **wasip1 has no socket syscalls, period.** Not "denied by default" — denied by construction. No `connect()`, no DNS, no TLS initiation. The guest cannot fabricate a socket. |
| Subprocess | No `fork`/`exec` in wasip1. Shell-out is unavailable at the host-ABI level. |
| Memory | `StoreLimitsBuilder::memory_size(256 << 20)` caps each Store at 256 MiB (tunable). `memory.grow` fails beyond the cap; wasm traps. |
| CPU (deterministic) | `Config::consume_fuel(true)` + `store.set_fuel(N)`. Fuel counts wasm instructions executed. Deterministic across runs — same render hits the same fuel-exhaustion point, always. |
| CPU (wall-clock) | `Config::epoch_interruption(true)` + background thread calling `engine.increment_epoch()` on a fixed tick. `store.set_epoch_deadline(K)` traps when the current epoch exceeds deadline. Cheap to check (compiled into every loop backedge). Non-deterministic but fast. |
| Stack overflow | `Config::max_wasm_stack(bytes)` caps the wasm-side stack. Default 512 KiB; lower for defense-in-depth. |
| Instance count / table bloat | `StoreLimitsBuilder::instances(N)`, `tables(N)`. Prevents wasm from inflating host memory via many small allocations. |

## What's enforced in Package code itself (belt and suspenders)

Even inside the sandbox, akua applies additional invariants on the Package's own code:

- **Path-traversal guard** on every plugin callable's path argument. `kcl_plugin::resolve_in_package` canonicalizes + asserts-under-package-dir + resolves symlinks. A Package that passes `"../../etc/passwd"` to `pkg.render(...)` gets a typed error, not a render. Absolute paths are accepted only when they fall under an `allowed_roots` entry the renderer registered — today, that's exactly the set of resolved `charts.*` deps (path-based dep dir or OCI cache dir for the blob we just pulled). Nothing else.
- **KCL language-level sandbox.** The KCL language itself has no `os.read`, `http.get`, env reads, or `time.now()`. A pure-KCL Package is deterministic by construction. This is upstream KCL's own invariant.
- **Plugin registry is closed.** Only akua-core can register plugins (`kcl_plugin::register` is pub but only called at akua startup). Packages cannot invent their own.
- **Strict render mode** (`--strict`): reject raw-string paths in plugin callables. Forces typed `charts.*` imports resolved via `akua.toml`. Default for `akua publish` and `akua serve`; optional for interactive `akua render`.

---

## What's NOT shipped yet

This is the current-state gap vs the target. See [docs/roadmap.md](roadmap.md) phases for timing.

| Guarantee | State today | Phase |
|---|---|---|
| Path-traversal rejection in plugin handlers | Shipped — `resolve_in_package` + `allowed_roots` | ✅ Phase 0 |
| `helm.template` / `kustomize.build` via WASM engines | Shipped — no shell-out, wasmtime-hosted | ✅ Phases 1 + 3 |
| Typed `charts.*` imports + lockfile digests | Shipped — path + OCI, replace override | ✅ Phase 2a, 2b A+B |
| `akua render --strict` rejects raw chart paths | Shipped — `E_STRICT_UNTYPED_CHART` | ✅ Phase 2b C |
| `akua verify` path-dep digest drift detection | Shipped — `PathDigestDrift` / `PathMissing` | ✅ Phase 2b C |
| Render worker wrapped in wasmtime | Native binary — no sandbox | Phase 4 |
| `akua serve` per-tenant isolation | Verb doesn't exist | Phase 5 |
| cosign keyed verification on OCI deps | Shipped — `[signing] cosign_public_key`, ECDSA P-256 | ✅ Phase 6 A |
| `akua publish` with cosign sign-by-default | Shipped — P-256 PKCS#8 PEM private keys | ✅ Phase 7 A |
| `akua pull` with manifest digest verify | Shipped | ✅ Phase 7 A |
| cosign keyless (fulcio + rekor) verification | Not implemented | Phase 6 B |
| SLSA v1 attestation generation on publish | Shipped — DSSE envelope, in-toto v1 statement | ✅ Phase 7 B |
| SLSA attestation chain walk on verify | Not implemented — needs keyless + `akua verify` recursion | Phase 7 C |
| Encrypted cosign private keys (passphrase / HSM) | Not implemented | Phase 7 C |
| Git dep checkout via `gix` | Shipped — pure Rust, no shell-out | ✅ Phase 2b C |
| Private-repo OCI auth (docker config / akua auth.toml) | Shipped — Basic + bearer PAT | ✅ Phase 2b C |
| Docker credential helpers | Not implemented — would require shell-out | Won't ship |

---

## Operational guidance today (pre-Phase 4)

Until `akua render` dispatches through the wasmtime worker, operators running akua on untrusted input should:

1. Run akua in a container / gVisor / Firecracker microVM per render.
2. Mount the Package directory read-only.
3. Mount a dedicated output directory writable.
4. Enforce CPU/memory/time limits at the container level.
5. No network namespace (or explicit deny-all network policy).

This is what most hosted build services already do for arbitrary-input jobs (GitHub Actions, Vercel builds, Cloudflare Workers build). Standard pattern.

After Phase 4, the above is no longer necessary — wasmtime enforces it at the process level, no container required.

---

## Why no shell-out, ever

A prior design considered keeping `helm.template` as shell-out for convenience, with a feature flag and clear "trusted input only" warnings. That design is **rejected**. Reasons:

1. **Opt-in security is not security.** If the flag defaults to safe but can be flipped by a single command, every hosted service will eventually flip it for a one-off, forget, and get hit. "Secure by default" means the unsafe path doesn't exist, not that it's one flag away.
2. **Shell-out inherits host privileges.** `helm` runs as the akua process's user with full `PATH`, env, cwd, network. Sandboxing individual subprocess invocations (seccomp, unshared namespaces) is possible but fragile, platform-specific, and hard to verify.
3. **WASM engines are the viable alternative.** Benchmarks at [docs/performance.md](performance.md) show KCL under wasmtime/WASI runs at ~2× native — comfortably inside the sub-100ms render budget. helm-engine-wasm prior work hit 20 MB WASM + 2.3s cold render. These are fine numbers.
4. **Removing shell-out forces the right engineering.** As long as shell-out is "available as an escape hatch," investment flows there instead of toward the WASM engines. Cutting it is what unblocks Phases 1 + 3.

The alternative — keep shell-out with lots of warnings — would ship a sandbox that has a hole in it. That's worse than shipping no sandbox; users would assume protection that doesn't exist.

---

## Related docs

- [CLAUDE.md — invariants](../CLAUDE.md) — the "Sandboxed by default. No shell-out, ever." rule
- [docs/roadmap.md](roadmap.md) — phased plan to the end-state
- [docs/performance.md](performance.md) — benchmarks demonstrating WASM viability
- [docs/embedded-engines.md](embedded-engines.md) — what ships inside the akua binary
