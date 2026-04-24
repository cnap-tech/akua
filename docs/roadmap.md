# Roadmap

> **North star:** akua is a **sandboxed-by-default** rendering substrate. Every render runs inside a wasmtime WASI sandbox with memory / CPU / wall-clock caps and capability-model filesystem preopens. No shell-out, ever. Untrusted Packages are safe to render on shared hosts.
>
> Invariant lives in [CLAUDE.md](../CLAUDE.md) ("Sandboxed by default. No shell-out, ever."). Detailed model in [docs/security-model.md](security-model.md).

---

## Release tracks

The roadmap is ordered by implementation phase, but releases cut across phases. This section lists the minimum boxes each release ships with. Phases below give the detail.

### v0.1.0 — alpha (candidate ready now)

**Target:** solo authors + internal CI pipelines. Users who trust their own Package inputs and want the author/render/publish/verify loop end-to-end.

**Shipped and load-bearing for v0.1.0:**

- CLI contract + ~25 verbs (`init`, `render`, `check`, `lint`, `fmt`, `test`, `dev`, `repl`, `add`, `remove`, `tree`, `lock`, `update`, `verify`, `diff`, `inspect`, `pack`, `push`, `sign`, `pull`, `publish`, `cache`, `auth`, `whoami`, `version`)
- Deterministic raw-YAML render writer + per-output sha256
- `Package.k` loader via `kcl_lang::API` + the `akua.*` KCL stdlib
- Helm + Kustomize WASM engines (no shell-out, ever)
- Typed `charts.*` deps over path / OCI / git with `replace` + lockfile digests
- Cosign keyed verify + SLSA v1 attestation on publish
- Full air-gap crypto loop: `akua pack` → `akua sign` → transfer → `akua verify --tarball` → `akua push --sig`
- Operational verbs: `akua cache`, `akua auth`, `akua lock [--check]`, `akua update [--dep]`

**Honest caveats shipped alongside v0.1.0:**

- **Render is not yet process-sandboxed.** CLAUDE.md's "sandboxed by default" invariant is aspirational at the process level — today the render path is native Rust + in-process WASM engines. Phase 4 delivers the per-render wasmtime isolation. Until then, operators running akua on untrusted Package input should containerize per-render (guidance in [docs/security-model.md](security-model.md#operational-guidance-today-pre-phase-4)).
- **Rego test runner + policy engine not implemented.** `akua test` covers KCL only.
- **Cosign keyless (fulcio + rekor) not implemented.** Keyed flow only.

**Exit gate for v0.1.0:** all items above shipped, benchmarks green, security-model.md matches reality, release notes call out the caveats explicitly.

### v0.2.0 — beta ("sandboxed by default" actually delivered)

**Target:** hosted build services, CI pipelines accepting PR-submitted Packages, in-browser dev loops loading third-party examples. Drops the "containerize yourself" caveat.

- Phase 4 — wasmtime-hosted `akua render` (~2-3 weeks). Delivers the CLAUDE.md invariant at the process level.
- `akua attest` + `akua verify --att` (queued under Phase 8.5 Planned). Offline DSSE/SLSA alongside the existing signature flow.

**Exit gate for v0.2.0:** every `akua render` runs inside wasmtime with memory/fuel/epoch caps + capability-model preopens. Native render path no longer exists.

### v0.3.0 — hosted multi-tenant

- Phase 5 — `akua serve` (~2-3 weeks). Single process handles N concurrent renders with per-tenant isolation.

### v0.4.0 — supply-chain completeness

- Phase 6 B — cosign keyless verify (fulcio + rekor).
- Phase 7 C follow-up — recursive attestation walk over transitive deps.
- Phase 7 D — HSM / cosign-native key formats.

### v0.5.0+ — policy + operator surface

- Policy engine phase (design open — regorus vs OPA→WASM).
- Phase 8 — Rego test runner (depends on policy engine).
- Phase 8 — `akua repl` Rego half (depends on policy engine).
- Phase 9 — `akua deploy`, `akua query`, `akua trace`, `akua policy` ("ship when there's demand").

---

## Current state (as of 2026-04-24)

**Shipped on `main`:**

- [x] CLI contract primitives — universal args, typed exit codes, agent auto-detection, structured errors
- [x] `akua.toml` + `akua.lock` parsers with round-trip tests over every example
- [x] `Package.k` loader via `kcl_lang::API` + `option("input")` inputs
- [x] Every verb listed under v0.1.0 above
- [x] Raw-YAML render writer — deterministic `NNN-kind-name.yaml` + per-output sha256
- [x] KCL plugin bridge — `plugin_agent: u64` function pointer, JSON-in / JSON-out
- [x] `akua.*` KCL stdlib (`ctx`, `helm`, `kustomize`, `pkg`) — typed options-schema pattern
- [x] `pkg.render()` post-eval sentinel composition
- [x] Signed + attested OCI distribution (publish / pull / pack / push / sign / verify / verify --tarball)
- [x] Benchmarks: native vs WASI-wasm (2× overhead — see [docs/performance.md](performance.md))

**Deliberately not shipped (violates the sandbox invariant):**

- [x] ~~`engine-helm-shell` feature~~ — never existed post-Phase 0
- [x] ~~`engine-kustomize-shell` feature~~ — never existed post-Phase 0
- [x] ~~Any `--unsafe-host` / `--engine=shell` escape hatch~~ — will never ship

---

## Phase 0 — Rip shell-out, harden the render path (SHIPPED)

Removes the security escape hatch entirely. Establishes sandbox-first as the default posture.

- [x] No `engine-helm-shell` / `engine-kustomize-shell` / `--unsafe-host` feature exists in Cargo.toml — shell-out invariant holds at compile time
- [x] Path-traversal guard in `kcl_plugin::resolve_in_package` — canonicalize + assert-under-package-dir + symlink resolution
- [x] Integration tests cover path-escape rejection
- [x] [docs/security-model.md](security-model.md) written — threat model, what's guaranteed, what's not
- [x] [CLAUDE.md](../CLAUDE.md) invariant text

**Exit gate:** ✅ akua-core builds without any shell-out feature; `pkg.render({ path = "../.." })` returns a typed error; security-model.md documents what's still aspirational (Phase 4 process sandbox).

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

## Phase 4 — Wasmtime-hosted `akua render` (2-3 weeks) — **blocks v0.2.0**

Sandbox becomes the default execution path for akua itself. User-invoked `akua render` wraps a wasip1-compiled `akua-render-worker` inside wasmtime. This delivers CLAUDE.md's "sandboxed by default" invariant at the process level — v0.1.0 documents it as aspirational until this phase ships.

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

## Phase 5 — `akua serve` multi-tenant (2-3 weeks) — **blocks v0.3.0**

HTTP front end for concurrent render requests. Per-request `Store` with preopens + limits. Depends on Phase 4 (per-render `Store` semantics).

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

### Phase 6 slice B — deferred — **targets v0.4.0**

- [ ] Keyless verify via sigstore-rs (Fulcio cert chain + Rekor transparency log)
- [x] SLSA v1 predicate generation on `akua publish` (shipped Phase 7 B)
- [x] `akua verify` walks the attestation chain — Package → deps (direct only today; transitive deferred to Phase 7 C follow-up)
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

### Phase 7 slice B — SLSA attestation (SHIPPED — 2026-04-22)

- [x] `slsa` module: in-toto v1 statement + SLSA v1 provenance predicate builder. Materials pulled from `akua.lock`; `buildType = https://akua.dev/slsa/publish/v1`; builder id keyed to the akua release.
- [x] `cosign::sign_dsse` / `verify_dsse`: DSSE v1 envelope sign + verify with PAE (Pre-Auth Encoding) binding `payloadType` into the signature so cross-envelope-type substitution is rejected.
- [x] `oci_pusher::push_attestation`: pushes the DSSE envelope as a `.att` sidecar at `sha256-<hex>.att` with media type `application/vnd.dsse.envelope.v1+json`.
- [x] `akua publish` auto-attests when signing is active; `--no-attest` disables independently of `--no-sign`. `PublishOutput.attestation_tag` surfaces the sidecar ref.

### Phase 7 slice C (partial — SHIPPED 2026-04-22)

- [x] `oci_puller::pull_attestation`: fetches the `.att` sidecar from a registry + returns the DSSE envelope bytes. 404 → Ok(None) so consumers can distinguish "publisher didn't attest" from transport errors.
- [x] `akua verify` attestation chain walk: for every OCI dep in `akua.lock`, when a cosign public key is configured, pulls + verifies the sidecar, asserts the SLSA subject digest matches the lockfile-pinned digest. Three new typed violations: `AttestationMissing`, `AttestationInvalid`, `AttestationSubjectMismatch`.

### Phase 7 slice C — encrypted keys (SHIPPED — 2026-04-22)

- [x] `cosign::sign_keyed_with_passphrase` + `sign_dsse_with_passphrase`: encrypted PKCS#8 PEM (`-----BEGIN ENCRYPTED PRIVATE KEY-----`) supported. Unencrypted path unchanged.
- [x] `akua publish` reads `$AKUA_COSIGN_PASSPHRASE`. No `--passphrase` CLI flag — argv leaks to `ps`.
- [x] Missing passphrase on encrypted key surfaces a clear error naming the env var.

### Phase 7 slice C — vendored deps (SHIPPED — 2026-04-22)

- [x] `akua publish` resolves non-path deps + embeds each chart tree at `.akua/vendor/<name>/` in the tarball. Resolver failures print a loud stderr warning — no silent un-vendored publishes.
- [x] Resolver consults `<workspace>/.akua/vendor/<name>/` before attempting network fetch. Offline-after-pull renders now succeed for a published Package with OCI or git deps.
- [x] End-to-end round-trip integration test: pack-with-vendor → unpack → offline resolve → assert nginx resolved from `.akua/vendor/` with matching digest.

### Phase 7 slice C — still deferred — **targets v0.4.0**

- [ ] Recursive attestation walk over transitive deps — a published Package's own deps must themselves be attested. Blocked on fixture Packages that attest their dep graph.
- [ ] HSM / cosign-native key formats (PKCS#11 / cosign-cli key ref) — targets v0.4.0.

**Exit gate (full phase):** Published Package round-trips `akua publish` → `akua pull` → `akua render` with cosign signatures validated at each hop. ✅ slice A covers the core round-trip; slice B adds SLSA + offline-render-from-published-digests on top.

---

## Phase 8 — Author surface (`akua test`, `akua dev`, `akua repl`)

Shipping incrementally alongside the core. `akua test` is live; the
rest ship when demand justifies the surface.

- [x] `akua test` — `test_*.k` / `*_test.k` runner. Files are evaluated via the same `PackageK` loader `akua render` uses; KCL `assert` + `check:` failures surface as per-file test failures. Structured JSON verdict + exit code 1 on any fail. (2026-04-22)
- [ ] Rego test runner (`*_test.rego`) — paired with the policy engine phase
- [x] Golden-file snapshot support for render-output tests — `akua test --golden` dir-diffs every `package.k`×`inputs*.yaml` combo against `snapshots/<pkg>/<stem>/`; `--update-snapshots` regenerates. (2026-04-22)
- [x] `akua dev` — file-watch hot-reload. `notify` + `notify-debouncer-mini`; `Rendered`/`RenderError` events stream to stdout (JSONL in agent mode). Watches per kept subdir non-recursively so `target/`/`node_modules/` monorepos don't exhaust `fs.inotify.max_user_watches`. Broken-pipe-aware. (2026-04-22) — apply-to-cluster deferred (needs kind driver).
- [~] `akua repl` — KCL half shipped (2026-04-24): accumulates submitted lines into a growing `.k` source, re-evaluates via `eval_source`, prints top-level YAML. Meta commands `.load / .reset / .show / .help / .exit`. Plain-line I/O (users wanting history wrap via `rlwrap`). Rego half deferred until the policy engine phase is designed.

---

## Phase 8.5 — Operational verbs

Glue that ships alongside the author loop but targets operators
rather than authors. Small, composable, agent-friendly.

- [x] `akua cache list | clear [--oci|--git] | path` — inventory + reclaim the content-addressed caches under `$XDG_CACHE_HOME/akua/{oci,git}` that `akua add` + `akua render` populate. Discriminated JSON shape `{action: list|clear|path, …}`. Ephemeral CI runners and disk-pressure triage without `rm -rf` guessing. (2026-04-23)
- [x] `akua auth list | add | remove` — manage `$XDG_CONFIG_HOME/akua/auth.toml` without hand-editing TOML. `add --username`/`--token` reads the secret from stdin (mirrors `docker login --password-stdin` — no secret on argv, no TTY dependency). `list` tags each entry with source ("akua" / "docker" / "both") and auth_kind, never echoing secrets. (2026-04-23)
- [x] `akua pack` — local-file sibling of `akua publish`. Writes the same deterministic `.tar.gz` to disk instead of pushing. Unlocks air-gap transfers, offline signing, and bit-diff archival. Defaults to `<workspace>/dist/<name>-<version>.tar.gz` (walker-skipped subdir so re-packing is idempotent); `--no-vendor` skips embedding deps. Emits `layer_digest` matching the OCI layer digest the registry would assign. (2026-04-23)
- [x] `akua push --tarball <path> --ref <oci://...> --tag <t>` — upload a pre-packed tarball. The push half of `akua publish`, decomposed so air-gap flows complete: pack here, transfer, push there. No signing / attestation (publish remains the all-in-one). (2026-04-24)
- [x] `akua inspect --tarball <path>` — triage a packed `.tar.gz` in-memory without unpacking. Reports `{package_name, version, edition}` parsed from the embedded `akua.toml`, `layer_digest`, `{compressed,uncompressed}_size_bytes`, `file_count`, sorted `vendored_deps`. Completes the air-gap triad: pack → transfer → inspect → push. (2026-04-24)
- [x] `akua lock [--check]` — regenerate `akua.lock` from `akua.toml` (cargo `generate-lockfile` analogue). `--check` diffs without writing and exits `E_LOCK_DRIFT` on staleness — pre-commit / CI gate to catch "author edited akua.toml but forgot to re-lock." Preserves signatures on unchanged entries via `merge_into_lock`; canonical TOML byte-compare for drift detection. (2026-04-24)
- [x] `akua update [--dep <name>]` — intentionally bump the lock against whatever upstream now serves. Inverse stance to `akua lock`: where `lock` rejects OCI digest drift (security), `update` accepts it and records the new digest. `--dep` scopes the lockfile write to one entry (cargo `update -p foo` analogue). Output lists `{updated, unchanged, skipped}` so operators see exactly what moved. (2026-04-24)
- [x] `akua sign` + `akua push --sig` — offline signing pair that completes the air-gap flow. `akua sign --tarball --ref --tag [--key]` computes `oci_pusher::compute_publish_digests()` locally (pure function, matches registry-side math post-push) and writes a `.akuasig` sidecar (JSON; carries `{oci_ref, tag, manifest_digest, simple_signing_payload, signature_b64, akua_version}`). `akua push --sig <path>` validates ref/tag/digest against the push target pre-upload, then pushes the `.sig` tag via the existing cosign push path. Sign + push hosts must pin the same akua binary (config blob embeds `env!("CARGO_PKG_VERSION")`). (2026-04-24)
- [x] `akua verify --tarball <path> [--sig <path>] [--public-key <path>]` — offline verify against a `.akuasig`, no registry round-trip. Three checks: sidecar readable, local manifest_digest matches sidecar's, ECDSA signature verifies (skipped when no public key). Falls back to `akua.toml [signing].cosign_public_key`. Closes the air-gap loop: pack → sign → transfer → verify → push. Full chain smoke-tested end-to-end. (2026-04-24)

### Planned

- [ ] `akua attest` + `akua push --att` — offline attestation pair symmetric to `akua sign`. Signs an SLSA v1 DSSE envelope bound to the tarball's manifest digest; sidecar format `.akuaatt` mirrors `.akuasig`. Completes the air-gap crypto story alongside signing.
- [ ] Extend `akua verify --tarball` with `--att <path>` — DSSE attestation verify. Lands with `akua attest`.

---

## Phase 9 — Deploy + operator surface — **post-v0.5.0**

`akua deploy`, `akua query`, `akua trace`, `akua policy` — cluster-facing operational verbs. Out of scope for the sandbox-first core; ship when there's demand.

---

## v0.1.0 release punch list

Concrete boxes to check before cutting the alpha tag. Everything under "core verbs + format" is already shipped per the phases above; this is the "are we actually ready to release?" gate.

### Build + test

- [ ] Release notes draft — what's in, what's caveated, what comes in v0.2.0
- [ ] `cargo test -p akua-core -p akua-cli` green on CI across Linux + macOS
- [ ] `akua --version` matches the tag
- [ ] Every `examples/<name>/` renders through `akua render` without errors
- [ ] Every `examples/<name>/` passes `akua check && akua lint && akua test`
- [ ] One curated upstream Package published to a public OCI registry for `akua pull` smoke-testing

### Docs sweep — sharpen + remove outdated claims

Many markdown files predate recent shipping and make claims that no longer match reality. A single focused pass before release, not phase-by-phase.

- [ ] **[CLAUDE.md](../CLAUDE.md)** — strip aspirational language that reads as current. Today the file says "every render executes inside a wasmtime WASI sandbox" and "untrusted Packages are safe to render on shared hosts" — both are Phase-4 goals, not v0.1.0 reality. Reword to `**target state**:` vs `**today**:` so new readers aren't misled. Same treatment for the capability table (`akua fmt` only handles `.k` today, not `.rego`; `akua test` is KCL-only; `akua repl` KCL-only; `akua lint` runs kcl-lint, not Regal yet; `akua policy` / `akua deploy` / `akua infra` / `akua audit` / `akua query` are Phase 9, not shipped).
- [ ] **README.md** (repo root) — reflect the shipped verb set (~25 today, not the "thirty verbs" line from CLAUDE.md). Remove phantom verbs. Single clear paragraph on the v0.1.0 caveats: process-sandbox is Phase 4, Rego layer is post-0.5.0.
- [ ] **[docs/cli.md](cli.md)** — list every shipped verb + every exit code they emit. Drop any entry for verbs that don't yet exist (`policy`, `deploy`, `query`, `trace`, `audit`, `infra`, `export`).
- [ ] **[docs/cli-contract.md](cli-contract.md)** — audit against what the universal args actually accept today. Every "MUST" must be true of every verb.
- [ ] **[docs/package-format.md](package-format.md)** — matches the `PackageK` loader's current parse (no drift since last doc refresh).
- [ ] **[docs/lockfile-format.md](lockfile-format.md)** — matches `AkuaLock::save` output byte-for-byte.
- [ ] **[docs/security-model.md](security-model.md)** — section "Operational guidance today (pre-Phase 4)" must call out the containerize-per-render recommendation prominently. Verify the feature-status table matches Cargo.toml.
- [ ] **[docs/embedded-engines.md](embedded-engines.md)** — list only engines that actually ship (helm + kustomize today). Mark OPA / Regal / CEL / Kyverno / kro as future.
- [ ] **[docs/policy-format.md](policy-format.md)** — if Rego isn't wired up yet, mark the whole doc as target-state with a prominent "NOT YET SHIPPED" banner.
- [ ] **[docs/agent-usage.md](agent-usage.md)** — lists the verbs agents are expected to invoke and the JSON shapes they should parse. Remove any verb that's not yet shipped.
- [ ] **[docs/performance.md](performance.md)** — benchmark numbers refreshed from main; drop shell-out baseline references if present.
- [ ] **[docs/impl-plan.md](impl-plan.md)** — cross-check against roadmap.md; remove duplication, or collapse to a pointer if this file has drifted past usefulness.
- [ ] **[docs/sdk.md](sdk.md)** — if the TypeScript SDK isn't shipped, mark as target-state or remove the reference from CLAUDE.md.
- [ ] **`examples/*/README.md`** — every example's README describes what it actually does today. Remove "shell-out" references; point to Phase 1/3 WASM engines.
- [ ] **Package author's README template** — `akua init` scaffolds a README that compiles on first `akua render`.

### Feature-docs for shipped surface

- [ ] Air-gap flow end-to-end: `akua pack` → `akua sign` → transfer → `akua verify --tarball` → `akua push --sig`, runnable snippet with a freshly-generated key.
- [ ] Publishing story: `akua publish` + `[signing]` config + what a consumer sees on `akua pull` + `akua verify`.
- [ ] Operational verbs crib sheet: `akua cache`, `akua auth`, `akua lock [--check]`, `akua update`.

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
