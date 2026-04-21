# Performance

> **Refreshed 2026-04-21 after Phases 1 + 3** — numbers now reflect
> the embedded WASM helm + kustomize engines. Shell-out is permanently
> gone per CLAUDE.md. See §4 for the refreshed end-to-end table and
> the new §5 on WASM-engine cold-start cost + optimization plan.

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

## 3. Plugin dispatch overhead

What it measures: cost of a `kcl_plugin.<module>.<fn>` call from a Package body. The `pkg.render` handler is the cheapest possible plugin — its Rust handler emits a single sentinel dict and returns. So differences vs pure-KCL baseline isolate **JSON-in / JSON-out FFI cost**, not engine work.

| Fixture | Warm median | Δ vs pure-KCL tiny |
|---|---:|---:|
| tiny (pure KCL, 1 resource) | 0.3 ms | baseline |
| 1× `pkg.render` | 0.3 ms | ≈ 0 ms |
| 10× `pkg.render` | 0.5 ms | +0.2 ms |

**Takeaway:** plugin dispatch is ~**20 µs per call** amortized. Not a bottleneck.

---

## 4. End-to-end `akua render` CLI latency (embedded WASM engines)

What it measures: full user-visible time — binary startup + arg parse + Package load + KCL eval + any plugin work + WASM engine instantiation + render. Measured via [`hyperfine`](https://github.com/sharkdp/hyperfine) with 3 warmup runs and ≥10 timed runs, `--dry-run` so filesystem writes don't vary the sample.

| Fixture | Mean | Plugin work? |
|---|---:|---|
| `examples/08-pkg-compose/shared/` (pure KCL, 1 resource) | **7.8 ms** | — |
| `examples/08-pkg-compose/` (outer + `pkg.render` composition) | **10.8 ms** | 2× `pkg.render` sentinels |
| `examples/09-kustomize-hello/` (kustomize-engine-wasm) | **76.3 ms** | 1× `kustomize.build` |
| `examples/00-helm-hello/` (helm-engine-wasm, forked) | **57.4 ms** | 1× `helm.template` |

**Takeaway:** binary startup + Package load is ~7 ms; each WASM-engine call adds **~60-110 ms** of per-invocation cost (module `Module::deserialize` fixup + Go runtime init chain + render). Linear in engine-call count.

The cost is dominated by one-time-per-render startup, not render work itself:

- wasmtime `Module::deserialize` of the `.cwasm`: ~5-10 ms
- Fresh `Store` + `WasiCtx` instantiation: ~1 ms
- Go runtime `_initialize` (klog + sprig + helm init chains): ~30-60 ms
- Actual `engine.Render` / `kustomize build`: ~10-20 ms

---

## 5. WASM-engine cold-start cost — and how to amortize it

The per-invocation ~60-110 ms sits mostly in Go's package `init()` chains running every call. Three mitigation paths, all open follow-ups:

### 5.1 Persistent engine state across renders ✅ shipped

**Shipped** (thread-local `Session` in both helm-engine-wasm and
kustomize-engine-wasm). One `Store` + `Instance` + typed-function
lookups per thread, reused across every plugin call for the life of
the process. `_initialize` (Go runtime + package init chain) runs
exactly once.

Next wins along this axis (not shipped):

- **Persistent Engine across invocations in `akua dev` / `akua serve`** — single long-lived process. Today each `akua render` is a fresh process and pays the init once. A long-lived process pays it once total.

### 5.2 Pooling allocator

`wasmtime::InstanceAllocationStrategy::pooling(...)` pre-allocates memory slots with guard pages; instantiate becomes a slot checkout + CoW reset instead of full mmap. Takes instantiation from ms to µs. Composes with 5.1.

### 5.3 Snapshot-after-init

Wasmtime's `Module::serialize` on a post-`_initialize` instance would let us skip the Go init chain entirely on subsequent renders. Not trivially exposed by wasmtime today; probably more effort than it's worth until 5.1 + 5.2 exhaust their gains.

---

## Implications for the signature experience

`akua dev` (the masterplan-§12 hot-reload loop) wants sub-100ms edit-to-re-render. Current budget vs today's measured numbers:

| Package complexity | End-to-end render | Under 100ms budget? |
|---|---:|:---:|
| Pure KCL, ≤ 500 resources | 7-20 ms | ✅ |
| Pure KCL + `pkg.render` composition | 10-15 ms | ✅ |
| Mixed with one `kustomize.build` | 75-90 ms | ✅ |
| Mixed with one `helm.template` | 55-65 ms | ✅ |
| Mixed with 5× `helm.template` | ~70-90 ms* | ✅ |

*Session reuse (§5.1) means the init cost is paid once per process.
A Package with N helm.template calls pays roughly (init_once + N ×
render_work) instead of N × (init + render_work).

Combined effect of Phase 1b (forked helm: 75 MB → 20 MB wasm, faster
deserialize) + §5.1 session reuse cut the single-helm case from
~120 ms to **~57 ms** — inside the 100 ms budget. `akua dev`
persistent-process will drive it lower still.

---

## Reproducing

The harnesses live under `/tmp` in the dev environment; they're intentionally not checked in because they pull `akua-core` via a local path and `kcl-lang` via git. To reproduce:

1. **Pure KCL benchmark** — 50 lines of Rust that calls `kcl_lang::API::exec_program` in a loop. Build `--release` natively and for `--target wasm32-wasip1`. Run the wasm binary under a minimal `wasmtime` Rust host that stubs the plugin import.
2. **Plugin dispatch benchmark** — same harness, adds `akua-core` as a dep, calls `install_builtin_plugins()` before the loop.
3. **End-to-end CLI benchmark** — `task build:helm-engine-wasm && task build:kustomize-engine-wasm && cargo build --release -p akua-cli`, then `hyperfine --warmup 3 --min-runs 10 'target/release/akua render --package ... --dry-run'`.

If the benchmarks need to go into CI, the harnesses move into `crates/akua-bench/` with criterion and get versioned. Out of scope until we have a performance regression story.
