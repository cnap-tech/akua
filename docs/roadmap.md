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

## Phase 1 — `helm-engine-wasm` restoration (1-2 weeks)

Research recommendation (see agent reports): revive akua's deleted fork, not vendor kclipper. kclipper uses Helm v3 with the heavier `action.Install` call path, never compiled to wasip1. Our prior work used Helm v4 + a ~100-line client-go strip + direct `engine.Render` — proven 20 MB WASM + 2.3s cold render + 1.4s warm (precompiled). Apache-2.0 on both ends; fork is cheap to maintain (~30 min per Helm release).

- [ ] Restore `crates/helm-engine-wasm/` from git history (`git show 0ade7f0^:…`)
- [ ] Resync fork to Helm v4.1.4 (latest stable at time of writing)
- [ ] Taskfile target `build:helm-engine-wasm` with cached output at `crates/helm-engine-wasm/assets/helm-engine.wasm`
- [ ] Wasmtime host in akua-core: load helm-engine.wasm at startup, expose `helm.template` plugin over it
- [ ] Plugin handler swaps in — same `akua.helm.Template` schema, same plugin name, just the backend changes
- [ ] Example 00-helm-hello renders again via the embedded engine (no `helm` on PATH required)
- [ ] Benchmark: confirm warm render stays under the 100ms dev-loop budget

**Exit gate:** `examples/00-helm-hello` renders end-to-end in a sandbox. `helm` on PATH is never consulted.

---

## Phase 2 — Typed `charts.*` deps via `akua.toml` (2-3 weeks)

Spec-to-code convergence. [docs/package-format.md §2](package-format.md) and [docs/lockfile-format.md](lockfile-format.md) already document `import charts.<name>` + OCI / Git / Path / Replace dep forms. The resolver doesn't exist yet.

- [ ] Extend `akua.toml` dep parser for `oci`, `git`, `path`, `replace` chart dep forms
- [ ] Resolver: local path → sha256 into `akua.lock`; OCI → pull + digest verify (no cosign yet — Phase 6)
- [ ] `akua.charts` stdlib module — resolver-produced typed `Chart` values exposed at `import charts.<name>`
- [ ] `helm.Template.chart: str | Chart` union type
- [ ] `--strict` render mode: reject raw-string chart paths (forces typed import)
- [ ] `akua add`, `akua remove`, `akua tree` grow chart-dep support
- [ ] Replace directive (`{ oci = "...", replace = { path = "../fork" } }`) honored — go-modules-style dev override
- [ ] `examples/01-hello-webapp` renders against a pinned OCI chart

**Exit gate:** `examples/01-hello-webapp` renders against a Verified OCI chart by digest. `akua render --strict` rejects any raw-string plugin path.

---

## Phase 3 — `kustomize-engine-wasm` (1-2 weeks)

Same pattern as Phase 1, different upstream. `sigs.k8s.io/kustomize/kustomize/v5` Go → wasm32-wasip1.

- [ ] `crates/kustomize-engine-wasm/` scaffold + `go-src/` wrapper
- [ ] Wasmtime host exposes `kustomize.build` plugin over the embedded engine
- [ ] Example 09-kustomize-hello renders via embedded engine
- [ ] Taskfile target `build:kustomize-engine-wasm`

**Exit gate:** `examples/09-kustomize-hello` renders without `kustomize` on PATH.

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

## Phase 6 — Supply chain (2-3 weeks)

- [ ] cosign verification on OCI deps via `sigstore-rs`
- [ ] SLSA v1 predicate generation on `akua publish`
- [ ] `akua verify` walks the attestation chain — Package → deps → transitive deps
- [ ] `akua.toml` `strictSigning: true` becomes default

**Exit gate:** a published Package with a `charts.*` dep round-trips through `akua publish` → `akua pull` → `akua render` → `akua verify`, all signatures validated.

---

## Phase 7 — Publishing + distribution (1-2 weeks)

With Phase 6 landing the crypto primitives, publishing is short work.

- [ ] `akua publish` — OCI push with cosign sig + SLSA attestation; vendor resolved deps into OCI layers
- [ ] `akua pull` — pull + verify + cache locally
- [ ] Dep resolver honors published digests

**Exit gate:** Packages publishable to any OCI registry, verifiable offline by `akua verify`, consumable by `akua render` with no network at render time.

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
