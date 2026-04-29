# Debugging akua

How to make the render pipeline cough up useful diagnostics when
something goes wrong. This is the playbook the maintainer uses; agents
should reach for it before guessing.

## TL;DR

```sh
akua render --package package.k --inputs ... --log=json --log-level=debug 2>&1 | head -20
```

Three knobs cover almost every case:

| knob | what it adds |
|---|---|
| `--log=json` | structured stderr lines you can `jq` over |
| `--log-level=debug` | host + worker spans, every plugin-bridge call |
| `RUST_LOG=...` | full `EnvFilter` syntax, lights up transitive crates |

## Sources of truth

- **CLI contract §9** — [docs/cli-contract.md](cli-contract.md#9-logging) — flag semantics, JSON line shape, target taxonomy.
- **CLI contract §9.1** — OpenTelemetry env-var surface.
- **`crates/akua-cli/src/observability.rs`** — host-side subscriber wiring.
- **`crates/akua-render-worker/src/observability.rs`** — worker-side subscriber.

## What you'll see

Successful render at `--log-level=debug`:

```json
{"level":"DEBUG","fields":{"message":"worker.invoke.start"},"target":"akua","span":{...,"name":"worker.invoke"}}
{"level":"DEBUG","fields":{"message":"bridge.call","method":"kcl_plugin.helm.template","args_len":131,"kwargs_len":2},"target":"akua::bridge",...}
{"level":"DEBUG","fields":{"message":"bridge.response","response_len":203,...},"target":"akua::bridge",...}
{"level":"DEBUG","fields":{"message":"kcl eval ok","worker_target":"akua::worker","fields_json":"{...,\"yaml_size\":272}"},"target":"akua::worker",...}
```

Three target namespaces:

- `akua` — host pipeline (verbs, render-worker.invoke span, chart resolver).
- `akua::worker` — events the worker emitted, replayed under the host's live `worker.invoke` span. The original target is preserved as `worker_target`.
- `akua::bridge` — KCL-plugin host-function calls (`bridge.call`, `bridge.response`).

## When wasmtime traps

The worker runs inside a wasip1 sandbox. Without the right config, traps surface as `wasm function NNNN` with no symbols. akua already enables symbolication globally; you just have to read it.

```text
worker trapped: error while executing at wasm backtrace:
   11: 0x389a8a - type_pack_and_check
                    at .../kcl/d584c0b/crates/evaluator/src/ty.rs:184:9
   12: 0x37a6c8 - walk_assign_stmt
                    at .../kcl/d584c0b/crates/evaluator/src/node.rs:110:21
```

Frame names + file:line resolve via:

- `Config::wasm_backtrace_details(Enable)` + `Config::generate_address_map(true)` in `engine_host_wasm::shared_config`. Backtrace capture itself is on by default in wasmtime 43.
- The worker `.wasm`'s `name` section preserved by overriding the workspace `[profile.release]` strip via the `task build:render-worker` `--config` flags (see `Taskfile.yml`).

If a trap shows bare `wasm function NNNN`:

1. The worker `.wasm` is stale or stripped — run `task build:render-worker` and verify the file size grew.
2. The `.cwasm` AOT artifact may be cached — `cargo clean -p akua-cli && cargo build -p akua-cli` to force a re-bake against the current Config.

## When a plugin handler fails

KCL plugin calls (`pkg.render`, `helm.template`, `kustomize.build`) cross from inside-the-sandbox guest code to host functions. Failures look like:

```json
{"code":"E_RENDER_KCL","message":"plugin panic: pkg.render: <details>"}
```

Workflow:

1. Run with `--log-level=debug`. The `bridge.call` event right before the failure shows the method, `args_len`, `kwargs_len`. Method names match the registered handler in `akua-core/src/{pkg_render,helm,kustomize}.rs`.
2. If the panic message is opaque (e.g. `i/o resolving \`\``), the handler probably hit an unexpected empty value — add a `tracing::debug!` near the failing read in the handler. Recompile, re-run.
3. The `bridge.response` event tells you whether the handler returned (envelope size) or trapped (no `bridge.response` follows the `bridge.call`).

`AKUA_BRIDGE_TRACE=1` is a back-compat env-var shortcut for `--log-level=debug` filtered to `akua::bridge`.

## Distinguishing host vs worker failures

| symptom | side | next step |
|---|---|---|
| `worker trapped: …` + symbolicated backtrace | worker (KCL eval, plugin reentry) | read the top frames; `kcl-evaluator/src/ty.rs:184` is `type_pack_and_check` panicking on a schema-vs-value mismatch |
| `plugin panic: <plugin-name>: <message>` | host (the plugin handler returned `Err`) | grep the message in `crates/akua-core/src/{pkg_render,helm,kustomize}.rs` |
| Structured error envelope only, no logs | host pre-render (chart resolve, lockfile, auth) | the verb's `RenderError` path; logs would have a `target = "akua"` event before the envelope if the verb itself emitted one |

## Replicating a failing render minimally

If a render fails inside an example:

```sh
cd examples/<name>
cargo run -q -p akua-cli -- render \
    --package package.k \
    --inputs inputs.example.yaml \
    --log=json --log-level=debug 2>&1 | tail -30
```

Tail (not head) grabs the failing event + envelope; head grabs the warm-up traffic. Pipe through `jq -c 'select(.level=="ERROR" or .target=="akua::bridge")'` to narrow to the salient lines.

## When the worker is the wrong version

A persistent gotcha: `cargo build -p akua-cli` does **not** rebuild the render-worker `.wasm`. The build script emits a warning when sources are newer than the staged `.wasm`:

```
warning: akua-cli@0.7.0: akua-render-worker.wasm is older than crates/akua-core/... — run `task build:render-worker`
```

Rebuild explicitly:

```sh
task build:render-worker
```

Worker-side changes that need this:

- Edits under `crates/akua-render-worker/`
- Edits under `crates/akua-core/` (the worker depends on it)
- Edits to bundled stdlib files under `crates/akua-core/stdlib/akua/*.k` (they're `include_str!`-embedded at compile time)

## Adding new spans + events

Convention used elsewhere in the codebase:

- Top-level verb work: `tracing::info_span!(target: "akua", "verb.<name>", …)`.
- Plugin / engine boundary: `tracing::debug!(target: "akua::bridge", "<event>", …)`.
- Worker-internal: `target: "akua::worker"`. The host's stderr-replay path forwards them under the live `worker.invoke` span.

Field hygiene:

- Names go in span fields (small, structured): `package_filename`, `chart_dep_count`, `kind`, `outcome`.
- Free-form text goes in the `message` (the macro positional arg).
- **Never log raw KCL inputs at info or warn** — they're user data. Log sizes (`source_size`, `inputs_kind`) instead. Full args are fine at debug for local repro.

## OpenTelemetry export

For cross-render or production traces, set `OTEL_EXPORTER_OTLP_ENDPOINT`. Example with a local collector:

```sh
docker run --rm -p 4317:4317 -p 16686:16686 jaegertracing/all-in-one
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 \
  OTEL_SERVICE_NAME=akua-dev \
  cargo run -q -p akua-cli -- render --package examples/00-helm-hello/package.k
```

The same trace tree (`worker.invoke → bridge.call → kcl eval`) shows up at `http://localhost:16686`. The OTel layer is gated on the `otel` cargo feature — on by default for the CLI binary, off for the napi distribution. cli-contract §9.1 lists every honored `OTEL_*` env var.

## Anti-patterns

Things that don't work and aren't worth trying:

- **Running KCL outside the worker for "easier debugging."** The worker preopens a curated filesystem slice; running `kcl run` directly against a Package will fail to import `akua.helm` / `akua.pkg` (the stdlib lives at `/akua-stdlib` only inside the worker). Reproduce inside the sandbox.
- **`println!` / `eprintln!` in plugin handlers.** They land on the host's stderr, but interleave unpredictably with the JSON-line subscriber. Use `tracing::debug!(target: "akua::bridge", …)` instead — same diagnostic, parented under the active span.
- **Quietly catching panics in plugin handlers.** The bridge already converts panics into a typed envelope (see `kcl_plugin::panic_envelope`); adding a second layer hides the wasm backtrace.
- **Filter directives leading with `debug` / `trace`.** Transitive crates (`kcl-evaluator`, `salsa`, `rustc_span`, `wasmtime_wasi`) emit hundreds of events per render. The default filter (`warn,akua=info,…`) keeps them silent. Override via `RUST_LOG` only when you need that context.
