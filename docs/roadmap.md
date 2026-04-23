# Roadmap

> **North star:** akua is a **sandboxed-by-default** rendering substrate. Every render runs inside a wasmtime WASI sandbox with memory / CPU / wall-clock caps and capability-model filesystem preopens. No shell-out, ever. Untrusted Packages are safe to render on shared hosts.
>
> Invariant lives in [CLAUDE.md](../CLAUDE.md) ("Sandboxed by default. No shell-out, ever."). Detailed model in [docs/security-model.md](security-model.md).

---

## Current state (as of 2026-04-21)

**Shipped on `main`:**

- [x] CLI contract primitives — universal args, typed exit codes, agent auto-detection, structured errors
- [x] `akua.toml` + `akua.lock` parsers with round-trip tests over every example
- [x] `Package.k` loader via `kcl_lang::API` + `option("input")` inputs
- [x] 13 verbs: `init`, `add`, `remove`, `tree`, `whoami`, `version`, `verify`, `check`, `lint`, `fmt`, `diff`, `inspect`, `render`
- [x] Raw-YAML render writer — deterministic `NNN-kind-name.yaml` + per-output sha256
- [x] KCL plugin bridge — `plugin_agent: u64` function pointer, JSON-in / JSON-out
- [x] `akua.*` KCL stdlib (`ctx`, `helm`, `kustomize`, `pkg`) — typed options-schema pattern
- [x] `pkg.render()` post-eval sentinel composition
- [x] Benchmarks: native vs WASI-wasm (2× overhead, acceptable for secure-by-default — see [docs/performance.md](performance.md))

**Deliberately not shipped (violates the sandbox invariant):**

- [ ] ~~`engine-helm-shell` feature~~ — removed Phase 0 below
- [ ] ~~`engine-kustomize-shell` feature~~ — removed Phase 0 below
- [ ] ~~Any `--unsafe-host` / `--engine=shell` escape hatch~~ — will never ship

**Not yet shipped (planned):**

- [ ] `akua publish`, `akua pull` — signed OCI distribution
- [ ] `akua test`, `akua dev`, `akua repl`, `akua deploy`, `akua query` — author + operator verbs
- [ ] OCI chart-dep resolver (typed `charts.*` imports)
- [ ] `akua serve` — hosted multi-tenant render

---

## Phase 0 — Rip shell-out, harden the render path (1-2 days)

Removes the security escape hatch entirely. Establishes sandbox-first as the default posture. Examples that use shell-out engines (00-helm-hello, 09-kustomize-hello) break temporarily until Phases 1 + 3 land their WASM replacements — documented in their READMEs.

- [ ] Delete `engine-helm-shell` feature + `crates/akua-core/src/helm.rs` (shell-out implementation)
- [ ] Delete `engine-kustomize-shell` feature + `crates/akua-core/src/kustomize.rs`
- [ ] Delete shell-out integration tests (`examples_helm_hello`, `examples_kustomize_hello`)
- [ ] Replace `helm.template` / `kustomize.build` plugin handlers with typed error returns (`E_ENGINE_NOT_AVAILABLE` — surface the specific engine + a pointer to the roadmap phase)
- [ ] Path-traversal guard in `kcl_plugin::resolve_against_package` — canonicalize + assert-under-package-dir + symlink resolution (port of `MacroPower/kclipper`'s `WithAllowedPaths`)
- [ ] Integration tests: `pkg.render({ path = "../../etc/passwd" })` rejected; symlink escape rejected
- [ ] Write [docs/security-model.md](security-model.md) — what's guaranteed, what's not, threat model
- [ ] Update [CLAUDE.md](../CLAUDE.md) invariant (done)
- [ ] Update `examples/00-helm-hello/README.md` + `examples/09-kustomize-hello/README.md` to point at Phase 1 / 3
- [ ] Update `docs/performance.md` — mark shell-out benchmarks as historical

**Exit gate:** akua-core builds without shell-out features; all non-shell-out examples render; path-traversal cases produce typed errors.

---

## Phase 1 — `helm-engine-wasm` restoration (SHIPPED — 2026-04-21)

Research recommendation: revive akua's deleted fork, not vendor kclipper. Prior work used Helm v4 + direct `engine.Render` — proven 20 MB WASM forked, 75 MB stock.

- [x] Restore `crates/helm-engine-wasm/` from git history — *cb…(Phase 1 commit)*
- [x] Go build works against stock Helm v4.1.4 via `-buildmode=c-shared` on wasip1
- [x] Taskfile target `build:helm-engine-wasm` — produces `crates/helm-engine-wasm/assets/helm-engine.wasm` (74 MB stock)
- [x] Wasmtime host in `crates/helm-engine-wasm/src/lib.rs` — loads `.cwasm`, renders via `pkg/engine.Render`
- [x] Plugin handler `crates/akua-core/src/helm.rs` — same `akua.helm.Template` schema, swaps in behind `engine-helm` feature
- [x] `examples/00-helm-hello` renders end-to-end via the embedded engine — **verified with `PATH=/nonexistent`**; byte-identical sha256 to prior shell-out render
- [x] Phase 1c: benchmark refreshed in `docs/performance.md`. Initial embedded-engine bench showed ~120 ms cold helm — Phase 1b + Phase 1d drove it to ~57 ms, inside the 100 ms dev-loop budget.
- [x] Phase 1b: `fork/apply.sh` + `task build:helm-engine-wasm` apply the client-go strip patch. Default build is forked (**20 MB** wasm, 73% smaller). `task build:helm-engine-wasm:stock` preserves access to the unpatched 75 MB variant for cross-checking.
- [x] Phase 1d: thread-local `Session` in both helm-engine-wasm + kustomize-engine-wasm. One Store + Instance + typed-func lookups reused across every plugin call for the life of the process. `_initialize` runs exactly once per thread. Multi-helm Packages now amortize to sub-100 ms (prior each call paid full init).

**Exit gate:** ✅ `examples/00-helm-hello` renders in a sandbox. `helm` on PATH never consulted. All Phase 1 boxes (1a + 1b + 1c + 1d) shipped. Cold render in ~57 ms.

- [x] Phase 1e: shared `engine-host-wasm` crate. Holds `Session` + `precompile` + `EngineSpec` + `thread_local_call`. `helm-engine-wasm` and `kustomize-engine-wasm` are now thin shims (serde types + tar helpers + engine-specific error wrapper). Future kro/CEL/kyverno engines get the wasmtime plumbing for free.

---

## Phase 2 — Typed `charts.*` deps via `akua.toml` (2-3 weeks)

Spec-to-code convergence. [docs/package-format.md §2](package-format.md) and [docs/lockfile-format.md](lockfile-format.md) already document `import charts.<name>` + OCI / Git / Path / Replace dep forms. The resolver is shipping in two slices.

### Phase 2a — local-path deps (SHIPPED — 2026-04-22)

- [x] `chart_resolver` module: local-path deps → canonicalized path + sha256 digest
- [x] Per-render `charts` KCL external pkg generated from resolved deps (`charts/<name>.k` exposes `path` + `sha256` constants)
- [x] `PackageK::render_with_charts` threads resolved chart paths as allowed absolute roots for the plugin path-escape guard — `helm.template(nginx.path, ...)` survives without an `--unsafe-host` escape hatch
- [x] `akua render` CLI verb auto-loads sibling `akua.toml`, resolves charts, passes them through
- [x] `examples/01-hello-webapp` vendored nginx chart + rewritten Package renders end-to-end — verified via `examples_hello_webapp.rs` integration test

### Phase 2b slice A — replace directive + lockfile digests (SHIPPED — 2026-04-22)

- [x] `replace = { path = "..." }` on OCI/git deps honored — pull source from the local fork while the lockfile still pins the canonical ref
- [x] `ResolvedSource` enum (`Path` / `Oci` / `OciReplaced` / `GitReplaced`) drives the lockfile writer
- [x] `chart_resolver::merge_into_lock` upserts path-dep + replace entries, preserving prior cosign / attestation metadata
- [x] `AkuaLock::save` / `find` / `upsert` writer API
- [x] `akua add` best-effort updates the lockfile on every edit
- [x] `akua verify` exempts `path+file://` sources from strict_signing

### Phase 2b slice B — OCI pull + digest verify (SHIPPED — 2026-04-22)

- [x] `oci_fetcher` module: HTTPS GET of manifest + chart blob, anonymous bearer-token dance for public ghcr.io / docker.io / quay.io
- [x] Content-addressed cache at `$XDG_CACHE_HOME/akua/oci/sha256/<hex>` — second render reuses the unpacked tree
- [x] Lockfile-pinned digest verify on pull — a drifted tag fails the render loudly with `LockDigestMismatch`
- [x] `ResolverOptions { offline, cache_root, expected_digests }` gate — `resolve()` stays offline for tests; `resolve_with_options()` is the network path
- [x] `akua render` + `akua add` pass lockfile digests as `expected_digests`
- [x] Integration test pulls `ghcr.io/stefanprodan/charts/podinfo:6.6.0`, caches, verifies digest-mismatch rejection

### Phase 2b slice C (SHIPPED — 2026-04-22)

- [x] `akua render --strict`: raw-string plugin paths rejected. Forces every chart to go through `akua.toml` + `import charts.<name>`. Typed exit code `E_STRICT_UNTYPED_CHART`.
- [x] `akua render --offline`: OCI / git deps must cache-hit. Air-gapped CI path.
- [x] `akua verify` path-dep drift detection: re-hashes vendored charts on disk, emits `PathDigestDrift` / `PathMissing` violations when the tree diverged from `akua.lock` or was deleted.
- [x] Git deps via `gix` (pure Rust, no shell-out). Clones into `$XDG_CACHE_HOME/akua/git/repos/` + checkouts under `checkouts/<sha>/`. Content-addressed, lockfile-pinned by commit SHA.
- [x] Private-repo OCI auth via `~/.config/akua/auth.toml` (akua-native TOML) and `~/.docker/config.json` (standard docker login format). Basic auth + bearer PATs supported; docker credential helpers intentionally not (shell-out).
- [x] Generated `charts.<name>` module grows a `Values` schema (from `values.schema.json`) + a `TemplateOpts` wrapper + `template()` lambda pre-filled with `chart = path`. Authors call `nginx.template(nginx.TemplateOpts { values = {...} })` — the "chart: str | Chart" ergonomic win, via a callable on the module rather than a schema union.
- [x] `akua remove` prunes matching lockfile entries; `akua tree` shows `[replace -> <path>]` markers for fork overrides.

**Exit gate:** ✅ all three slices shipped. OCI chart end-to-end via `akua render` (cache hit on second call). Git chart via `gix` (no shell-out). Private-repo auth for both. `--strict` / `--offline` / path-dep drift for CI-grade guarantees. `charts.<name>.template(...)` gives Package authors an autocomplete-driven authoring surface.

---

## Phase 3 — `kustomize-engine-wasm` (SHIPPED — 2026-04-21)

Same pattern as Phase 1, different upstream. `sigs.k8s.io/kustomize/api` +
`sigs.k8s.io/kustomize/kyaml` Go → `wasm32-wasip1`. Tar.gz sent over
linear memory; guest unpacks into `filesys.MakeFsInMemory()` so there
are no host-side preopens to grant.

- [x] `crates/kustomize-engine-wasm/` scaffold — Cargo.toml, build.rs, src/lib.rs, go-src/main.go
- [x] Wasmtime host in `src/lib.rs` exposes `render_dir` / `render_tar` API
- [x] `crates/akua-core/src/kustomize.rs` plugin handler — same `akua.kustomize.Build` schema, swaps in behind `engine-kustomize` feature (default-on)
- [x] `examples/09-kustomize-hello` renders end-to-end — **verified with `PATH=/nonexistent`**; byte-identical sha256 to prior shell-out render
- [x] Taskfile target `build:kustomize-engine-wasm` (26 MB stock wasm)
- [x] Integration test `tests/examples_kustomize_hello.rs` restored

**Exit gate:** ✅ `examples/09-kustomize-hello` renders in a sandbox. `kustomize` on PATH never consulted.

---

## Phase 4 — Wasmtime-hosted `akua render` (2-3 weeks)

Sandbox becomes the default execution path for akua itself. User-invoked `akua render` wraps a wasip1-compiled `akua-render-worker` inside wasmtime.

- [ ] Upstream KCL fixes: `uuid` `features = ["js"]` on wasm32; `kcl-language-server` gated with `#[cfg(not(target_arch = "wasm32"))]` around `lsp_types::Url::{to_file_path, from_file_path}` — file two PRs
- [ ] **Fork `kcl-lang/kcl` as `cnap-tech/kcl` if PRs stall** (expected); branch `akua-wasip1` maintained against upstream
- [ ] `akua-render-worker` binary targeting `wasm32-wasip1`
- [ ] AOT-compile `.cwasm` at akua's build time; embed in akua binary
- [ ] Wasmtime host harness: `InstanceAllocationStrategy::pooling(...)` + `Config::consume_fuel(true)` + `Config::epoch_interruption(true)` + `StoreLimitsBuilder::memory_size(256 << 20)` + preopens only
- [ ] `akua render` dispatches through worker by default. No opt-out.
- [ ] Benchmark regression suite: sub-100ms render budget still met
- [ ] CVE-2026-34988 mitigation: pin `wasmtime >= 43.0.1`, keep `memory_guard_size` default, set `memory_reservation >= 4 GiB`

**Exit gate:** every `akua render` runs inside wasmtime. Native code path no longer exists for render execution.

---

## Phase 5 — `akua serve` multi-tenant (2-3 weeks)

HTTP front end for concurrent render requests. Per-request `Store` with preopens + limits.

- [ ] `akua serve` verb — HTTP API + worker pool sized by CPU count
- [ ] Per-request preopens: tenant's Package dir (read) + output dir (write) only
- [ ] Per-request resource caps: memory, fuel, epoch deadline
- [ ] `POST /render` with package URL/digest + inputs; returns rendered manifests + summary
- [ ] Observability: histogram of render times, rejection reasons (fuel, epoch, memory, invariant), per-tenant metrics
- [ ] Docs: deployment guide for hosted render

**Exit gate:** single `akua serve` process handles N concurrent renders against isolated tenant Packages, with hard resource caps and structured rejection on violation.

---

## Phase 6 — Supply chain

### Phase 6 slice A — keyed cosign verify (SHIPPED — 2026-04-22)

- [x] `cosign` module: ECDSA P-256 keyed verification of simple-signing payloads, digest correlation with the fetched manifest.
- [x] `oci_fetcher::fetch_with_opts` pulls the `sha256-<hex>.sig` sidecar + payload blob when a public key is configured; surfaces `CosignVerify` / `CosignSignatureMissing` distinctly.
- [x] `akua.toml [signing] cosign_public_key = "./keys/cosign.pub"` config. `ResolverOptions.cosign_public_key_pem` threads through to the fetcher. `akua render` loads the key off disk.
- [x] Typed CLI code `E_COSIGN_VERIFY` — agents branch on "bytes failed the supply-chain gate" separately from "couldn't resolve the chart."

### Phase 6 slice B — deferred

- [ ] Keyless verify via sigstore-rs (Fulcio cert chain + Rekor transparency log)
- [ ] SLSA v1 predicate generation on `akua publish` (Phase 7 dependency)
- [ ] `akua verify` walks the attestation chain — Package → deps → transitive deps
- [ ] `akua.toml` `strictSigning: true` makes the signing block mandatory on every OCI dep

**Exit gate (full phase):** A published Package with a `charts.*` dep round-trips through `akua publish` → `akua pull` → `akua render` → `akua verify`, all signatures validated. Slice A lands keyed verify; slice B closes the loop with keyless + SLSA once `akua publish` exists.

---

## Phase 7 — Publishing + distribution

### Phase 7 slice A (SHIPPED — 2026-04-22)

- [x] `oci_transport` module: shared HTTP + bearer-challenge auth. Fetcher + puller + pusher all funnel through it.
- [x] `oci_pusher` module: monolithic upload of blob + config + manifest under akua-specific media types (`application/vnd.akua.package.content.v1.tar+gzip`).
- [x] `akua publish --ref <oci://…> [--tag] [--no-sign]`: deterministic workspace tarball → OCI artifact. `package_tar::pack_workspace` excludes render outputs + hidden dirs + per-consumer `inputs.yaml`.
- [x] `oci_puller` module: inverse of pusher, enforces akua media type + manifest-declared digest.
- [x] `akua pull --ref <oci://…> --tag <v> --out <dir>`: fetches + `package_tar::unpack_to` into a target directory.
- [x] Cosign signing primitive: `cosign::build_simple_signing_payload` + `sign_keyed` (P-256 ECDSA), round-trip proven against the verify primitive Phase 6 A shipped.
- [x] `oci_pusher::push_cosign_signature`: pushes the `.sig` sidecar at `sha256-<hex>.sig` with `dev.cosignproject.cosign/signature` annotation.
- [x] `akua.toml [signing].cosign_private_key`: `akua publish` signs by default when set. `--no-sign` CLI override.
- [x] Typed exit codes `E_PUBLISH_FAILED` / `E_PULL_FAILED`.

### Phase 7 slice B — deferred

- [ ] SLSA v1 predicate generation on `akua publish` (Phase 6 B chain-walk dep)
- [ ] Vendor resolved deps into OCI layers so pull is network-free
- [ ] Encrypted private keys (passphrase / cosign native format)
- [ ] `akua verify` recursively walks a pulled Package's `charts.*` deps

**Exit gate (full phase):** Published Package round-trips `akua publish` → `akua pull` → `akua render` with cosign signatures validated at each hop. ✅ slice A covers the core round-trip; slice B adds SLSA + offline-render-from-published-digests on top.

---

## Phase 8 — Author surface (`akua test`, `akua dev`, `akua repl`)

Defer until 0-7 complete. These are signature-experience polish, not load-bearing for the sandbox story.

- [ ] `akua test` — `test_*.k` + `*_test.rego` runners; golden-file snapshot tests
- [ ] `akua dev` — file-watch hot-reload per masterplan §12; <500ms edit-to-applied against local kind cluster
- [ ] `akua repl` — interactive KCL / Rego shell

---

## Phase 9 — Deploy + operator surface

`akua deploy`, `akua query`, `akua trace`, `akua policy` — cluster-facing operational verbs. Out of scope for the sandbox-first core; ship when there's demand.

---

## Non-goals (forever, recorded here for future-us)

- ❌ **Shell-out in the render path.** Invariant. No feature flag, no `--unsafe-host`, no exceptions. If an engine isn't WASM-ready, the feature doesn't ship until it is.
- ❌ Non-Kubernetes deploy targets (Fly / Workers / Lambda / systemd). akua is Kubernetes-native rendering; other substrates can consume the rendered YAML.
- ❌ Cluster-side controllers for akua kinds. akua renders; reconcilers (ArgoCD / Flux / kro / Crossplane) apply.
- ❌ Runtime (in-cluster) rendering. Render at CI time; git contains what runs.
- ❌ Curated central package catalog. First-party publishing by each maintainer is the model; akua provides signing + distribution infrastructure, not the shelves.
- ❌ PaaS / hosted runtime. akua is a toolkit; CNAP (and similar) is the hosted platform that consumes akua Packages.

---

## Checklist conventions

- `[ ]` — not started
- `[-]` — in progress
- `[x]` — shipped on `main`
- `[~]` — shipped, known follow-ups outstanding

When checking a box, link the commit: `[x] Task name — abc1234`.
