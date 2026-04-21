# Performance

> **Historical note.** Sections **3** (engine callable cost) and **4**
> (end-to-end `akua render` CLI latency — helm/kustomize rows) measured
> the shell-out Helm/kustomize backends, which were deleted in Phase 0
> of [`docs/roadmap.md`](roadmap.md). akua no longer shells out, ever.
> Numbers for the embedded WASM backends land when Phases 1 + 3 ship.
> Pure-KCL + `pkg.render` numbers (sections 1, 2) are current and valid.

Render-path benchmarks. Useful for:

- Sanity-checking that akua's pipeline is fast enough for the signature experience (`akua dev` sub-100ms edit-to-render loops).
- Understanding the cost of each engine callable (`helm.template`, `kustomize.build`, `pkg.render`) so Package authors can reason about their render budget.
- Validating that the WASI WebAssembly target (for shipping-a-renderer / ArgoCD plugin use cases) is within an acceptable latency multiplier vs native.

All numbers are in milliseconds. Measured on one machine; absolute values will vary. The *ratios* are the part that generalizes.

---

## Test bench

| | |
|---|---|
| CPU | Apple M4 Pro (12 cores) |
| RAM | 48 GB |
| OS | macOS 26.3.1 |
| Rust | 1.93.0 (release + LTO + strip) |
| KCL | `kcl-lang` 0.12.3 |
| wasmtime | 43.0.1 |
| helm | 4.1.1 |
| kustomize | (via mise, 5.8.1) |
| hyperfine | 1.20.0 |

All builds `--release`, `lto = "thin"`, `codegen-units = 1`, stripped.
Each timed run is post-warmup (1 cold + 1 warmup render before the measured loop, 20-50 iterations, all statistics over measured iterations only).

---

## 1. Pure KCL evaluation

What it measures: `kcl_lang::API::exec_program(...)` round-trip — parse + type-check + evaluate + serialize result to YAML. No plugin calls, no disk I/O on the output path. Measured via a direct Rust harness (no CLI startup).

Three fixtures:

- **tiny** — 1 resource, no plugin imports. `schema Input: greeting: str = "hi"`.
- **medium** — 20 resources, richer data (labels, annotations, small payloads).
- **large** — 500 resources, richest shape.

| Fixture | Native cold | Native warm (median) | WASI cold | WASI warm (median) | Warm overhead |
|---|---:|---:|---:|---:|---:|
| tiny (1 resource) | 5 ms | 0.3 ms | 3 ms | < 1 ms | ~same |
| medium (20 resources) | 1 ms | 1.0 ms | 2 ms | 1 ms | 1× |
| large (500 resources) | 21 ms | 16.5 ms | 32 ms | 33 ms | **2.0×** |

**Takeaway:** WASI wasm rendering through wasmtime runs at roughly **2× native speed** for large Packages. Small/medium Packages land in the same ballpark (sub-millisecond differences get dominated by timer resolution).

### wasmtime startup costs (one-time)

| Cost | JIT (`.wasm`) | AOT (`.cwasm` via `wasmtime compile`) |
|---|---:|---:|
| Module load / compile | ~540 ms | ~8 ms |
| Instantiate | < 1 ms | < 1 ms |
| Artifact size | 8.7 MB | 32 MB |

**Takeaway:** AOT-compile at build time cuts first-request latency from ~570ms to ~57ms. For anything longer-running than a one-shot CLI invocation, always AOT.

---

## 2. Plugin dispatch overhead

What it measures: cost of a `kcl_plugin.<module>.<fn>` call from a Package body. The `pkg.render` handler is the cheapest possible plugin — its Rust handler emits a single sentinel dict and returns. So differences vs pure-KCL baseline isolate **JSON-in / JSON-out FFI cost**, not engine work.

Native only (WASI plugins are stubbed — benchmarking stubs is meaningless).

| Fixture | Warm median | Δ vs pure-KCL tiny |
|---|---:|---:|
| tiny (pure KCL, 1 resource) | 0.3 ms | baseline |
| 1× `pkg.render` | 0.3 ms | ≈ 0 ms |
| 10× `pkg.render` | 0.5 ms | +0.2 ms |

**Takeaway:** plugin dispatch is ~**20 µs per call** (amortized from the 10× column). It's not a bottleneck for any realistic Package. The ABI is JSON-in / JSON-out via a function pointer, leaked `CString` return — no locking, no allocations beyond the payload.

---

## 3. Engine callable cost

What it measures: cost per invocation of a real engine callable, including subprocess spawn + external binary runtime. Native only (shell-out by definition).

Fixture: one `helm.template` call rendering a trivial in-tree chart (1 ConfigMap template) or one `kustomize.build` of a minimal overlay.

| Callable | Warm median | Dominant cost |
|---|---:|---|
| `helm.template` (1 chart) | 27 ms | `helm` subprocess (fork + exec + stdin + parse stdout) |
| 5× `helm.template` sequential | 143 ms | ≈ 28 ms × 5 — linear |
| `kustomize.build` (small overlay) | (included in CLI figure below) | `kustomize` subprocess |

**Takeaway:** each `helm.template` call costs ~28ms of subprocess overhead. Packages that call `helm.template` N times pay linearly. The embedded-WASM helm engine (masterplan §11 — `helm-engine-wasm`) exists to reclaim this; until then, minimize shell-out calls or parallelize them (not yet implemented).

---

## 4. End-to-end `akua render` CLI latency

What it measures: full user-visible time — binary startup + arg parse + Package load + KCL eval + any plugin work + summary emit. Measured via [`hyperfine`](https://github.com/sharkdp/hyperfine) with 3 warmup runs and ≥10 timed runs, `--dry-run` so filesystem writes don't vary the sample.

| Fixture | Mean | Plugin work? |
|---|---:|---|
| `examples/08-pkg-compose/shared/` (pure KCL, 1 resource) | **6.8 ms** | — |
| `examples/08-pkg-compose/` (outer + `pkg.render` composition) | 10.1 ms | 2× `pkg.render` sentinels |
| `examples/09-kustomize-hello/` (kustomize.build) | 16.8 ms | 1× `kustomize build` |
| `examples/00-helm-hello/` (helm.template) | 37.9 ms | 1× `helm template` |

**Takeaway:** binary startup adds ~6ms on top of the pure-KCL-eval numbers from §1. The `akua render` CLI floor on this machine is **~7ms** for pure-KCL Packages. Each `helm.template` shell-out adds ~28ms; each `kustomize.build` adds ~10ms.

---

## Implications for the signature experience

`akua dev` (the masterplan-§12 hot-reload loop) wants sub-100ms edit-to-re-render. Budget:

| Package complexity | End-to-end render | Headroom under 100 ms |
|---|---:|---:|
| Pure KCL, ≤ 500 resources | 7-20 ms | 80-93 ms |
| Pure KCL + `pkg.render` composition | 10-15 ms | 85-90 ms |
| Mixed with one `kustomize.build` | 16-25 ms | 75-84 ms |
| Mixed with one `helm.template` | 37-50 ms | 50-63 ms |
| Mixed with 5× `helm.template` | 150+ ms | **over budget** |

Packages heavy on helm shell-out (many charts) will miss the 100ms dev-loop budget until the embedded helm engine lands. Everything else is well within budget, including WASI-hosted rendering (would add ~16 ms on top of the native figures in the worst case).

---

## Reproducing

The harnesses live under `/tmp` in the dev environment; they're intentionally not checked in because they pull `akua-core` via a local path and `kcl-lang` via git. To reproduce:

1. **Pure KCL benchmark** — 50 lines of Rust that calls `kcl_lang::API::exec_program` in a loop. Build `--release` natively and for `--target wasm32-wasip1`. Run the wasm binary under a minimal `wasmtime` Rust host that stubs the plugin import.
2. **Plugin dispatch benchmark** — same harness, adds `akua-core` as a dep, calls `install_builtin_plugins()` before the loop.
3. **End-to-end CLI benchmark** — `cargo build --release --features "engine-helm-shell engine-kustomize-shell" -p akua-cli`, then `hyperfine --warmup 3 --min-runs 10 'akua render --package ... --dry-run'`.

If the benchmarks need to go into CI, the harnesses move into `crates/akua-bench/` with criterion and get versioned. Out of scope until we have a performance regression story.
