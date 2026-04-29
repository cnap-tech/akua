# akua CLI contract

> **Every verb honors this contract.** No exceptions. No verb exempts itself from any clause.

This document specifies the universal invariants every `akua` subcommand must satisfy. It is the foundation for agent-friendly operation. A verb that violates this contract is a bug, not a feature.

---

## 1. Output

### 1.1 `--json` is universal

Every verb accepts `--json` and emits a single, parseable JSON document (or JSON-lines stream for long-running commands) to stdout. No exceptions.

```sh
akua render --json
akua diff a b --json
akua deploy status --handle=r-4f2 --json
```

Without `--json`, verbs emit human-readable text to stdout. With `--json`, they emit structured data agents can parse.

### 1.2 Structured errors on stderr

Errors always go to stderr. With `--json`, errors are JSON-lines (one error per line). Without `--json`, errors are human-readable but still prefixed with a stable error code.

```
{"level":"error","code":"E_SCHEMA_INVALID","path":"apps/api/inputs.yaml","field":"replicas","message":"expected integer, got string","suggestion":"remove quotes around 3","docs":"https://akua.dev/errors/E_SCHEMA_INVALID"}
```

Every error has:

- `code` — stable, machine-readable identifier (SHOUTY_SNAKE_CASE)
- `message` — human-readable summary
- `path` — file, resource, or field that caused the error (if applicable)
- `suggestion` — actionable fix (if known)
- `docs` — URL to documentation (if available)

### 1.3 Determinism

Same inputs produce byte-identical output. Includes:

- JSON output key ordering (alphabetical by default; override with `--preserve-order` where meaningful)
- YAML output: fields sorted per the YAML style guide
- Rendered manifests: stable ordering by `kind/name`
- No timestamps in output unless explicitly requested (`--timestamps`)
- No non-deterministic IDs, random suffixes, or time-dependent values

### 1.4 Quiet by default under `--json`

Human-facing progress (spinners, color, banners) is suppressed when `--json` is set. Logs go to stderr as JSON-lines if `--log=json`. A `--verbose` flag may add more detail; it must not change the output format.

### 1.5 Agent context auto-detection

When `akua` is invoked inside an AI-agent session, it detects this from the environment and implicitly enables agent-friendly output defaults. The user never has to remember `--json` when an agent runs the command.

**Detection sources**, checked at process start in this order:


| env var set         | agent                                                    |
| ------------------- | -------------------------------------------------------- |
| `AGENT=<name>`      | emerging standard — Goose, Amp, Codex, Cline, OpenCode   |
| `CLAUDECODE=1`      | Claude Code                                              |
| `GEMINI_CLI=1`      | Gemini CLI                                               |
| `CURSOR_CLI=1`      | Cursor CLI                                               |
| `AKUA_AGENT=<name>` | akua-specific fallback for agents we haven't yet matched |


If any of these are set, the invocation is considered to be running in an **agent context**. Individual agents may set additional identifier variables (`GOOSE_TERMINAL`, `AMP_THREAD_ID`, `CODEX_SANDBOX`, `CLINE_ACTIVE`); we key off the primary marker above and record the secondary ones as context.

**What auto-enables when an agent is detected:**

- `--json` output (equivalent to passing the flag explicitly)
- `--log=json` (structured logs to stderr)
- `--no-color` (colors off; implicit under `--json` anyway)
- `--no-progress` (no spinners, no animated output)
- `--no-interactive` (prompts fail fast with exit code 1 and a clear error instead of blocking on stdin)

**Override semantics** (explicit always wins):


| invocation                                      | result                        |
| ----------------------------------------------- | ----------------------------- |
| `akua render --json` in a human shell           | JSON — flag wins              |
| `akua render --no-json` in an agent context     | text — explicit opt-out wins  |
| `akua render --format=text` in an agent context | text — explicit override wins |
| `akua render` in a human shell                  | text — default                |
| `akua render` in an agent context               | JSON — auto-detected          |


**No signal, by design.**

When detection activates, akua adapts behavior silently. No banner. No stderr announcement. No prelude on stdout. The behavior change is observable from the output itself (JSON vs text); agents that set `CLAUDECODE=1` or `AGENT=goose` already know they're in an agent context — akua repeating it back is noise.

Detection is introspectable when needed:

- `akua whoami --json` includes an `agent_context` field with the detected agent name and source env var.
- `--log-level=debug` emits a single `agent_context_detected` event in debug logs — useful for post-hoc diagnosis, silent in normal operation.

Otherwise: invisible by default, discoverable on demand. That's the discipline.

**Opt-out:**

- `AKUA_NO_AGENT_DETECT=1` — disable detection globally (useful for testing human-like output in an agent context, or for CI systems that happen to set agent env vars).
- `--no-agent-mode` — per-invocation override.

**Telemetry (when opted in):**

The detected agent name is included as an anonymized aggregate in telemetry records (`akua telemetry show` reveals the exact data). Never includes user data, prompts, or file contents — only the agent identifier string. Helps us see which agents are adopting akua and where to invest in compatibility.

**Why this is in the contract:**

The contract's goal is that agents drive akua reliably with minimal ceremony. Requiring `--json` on every invocation is ceremony. Detecting the context and doing the right thing is not. Same discipline as §4 (plan mode), §3 (idempotency keys), §6 (stable IDs) — default-on behaviors that make agent operation pleasant without requiring the agent to remember boilerplate.

Humans running akua in a terminal never notice the detection; their env vars don't match, and the CLI behaves as it always has.

---

## 2. Exit codes

Typed. Seven stable codes. Verbs do not invent their own.


| code | name           | meaning                                               |
| ---- | -------------- | ----------------------------------------------------- |
| 0    | success        | operation completed as requested                      |
| 1    | user error     | invalid inputs, bad flags, missing required arguments |
| 2    | system error   | unexpected failure (disk, network, bug)               |
| 3    | policy deny    | policy engine rejected the operation                  |
| 4    | rate limited   | registry / API rate limits                            |
| 5    | needs approval | operation is allowed but requires human approval      |
| 6    | timeout        | operation did not complete within --timeout           |


Any other exit code is a bug. Agents branch on these codes.

---

## 3. Writes are idempotent

Every verb that modifies state accepts `--idempotency-key=<uuid>`. If the same key is seen twice on the same resource with the same intent, the second call is a no-op and returns the original result.

- `akua deploy --idempotency-key=<k>` — safe to retry
- `akua publish --idempotency-key=<k>` — duplicate publish returns the original digest
- `akua secret rotate --idempotency-key=<k>` — rotating with the same key is idempotent

Agents generate fresh UUIDs per logical operation and retry on network errors without risk.

---

## 4. Plan mode

Every verb that modifies state accepts `--plan`. With `--plan`, the verb computes what it would do and emits the plan to stdout (JSON when `--json`, text otherwise) — but performs no writes.

```sh
akua deploy --plan
# → [JSON describing the manifests that would be applied,
#    owners, policy verdicts, diff vs current state]
```

Agents use `--plan` to reason about impact before committing. Plan output is deterministic.

---

## 5. Time bounds

Every verb that blocks on network or reconciliation accepts `--timeout=<duration>` (Go duration format: `30s`, `5m`, `1h`, `250ms`). Verbs never hang indefinitely.

- Default timeout is verb-specific but never more than 5 minutes.
- `--timeout=0` means "return immediately with current state" (for status-read ops).
- Timeouts exit with code 6.
- Invalid duration strings (`5min`, `2 hours`, raw integers) fail at parse time with `code=E_INVALID_FLAG`. Accepted units: `ns`, `us` / `µs`, `ms`, `s`, `m`, `h`.

`akua render` additionally honors `--max-depth=<N>` to cap the `pkg.render` composition chain (default 16). Hitting the cap fails with `E_RENDER_BUDGET_DEPTH`. Pair with `--timeout` for hardened CI / agent runs.

Async operations (`deploy`, `rollout`, long-running renders) return an opaque handle immediately; use `akua … wait --handle=<h>` to block.

---

## 6. Stable identifiers

Every resource has two identifiers:

- **Human name** — readable, mutable, scoped to namespace/environment.
- **Content-addressable ID** — `sha256:…` digest, immutable, globally unique.

Agents track by content-addressable ID. Humans read names. Both are always present in JSON output.

---

## 7. Discoverability

### 7.1 `akua help --json`

Returns a machine-readable tree of all verbs and subcommands with their flags, argument signatures, and one-line descriptions.

```sh
akua help --json
# → {"verbs":[{"name":"render","flags":[...],"subcommands":[...]},...]}
```

Agents parse this to discover capabilities without scraping man pages or markdown.

### 7.2 `akua <verb> --help`

Human-readable help for a single verb. Includes synopsis, flags, examples, links to docs.

### 7.3 `akua <verb> --describe --json`

Same data as `akua help --json` filtered to one verb. Useful for targeted introspection.

---

## 8. Authentication

- `akua login <registry>` authenticates to an OCI registry. Credentials are stored in the system credential store (Keychain on macOS, libsecret on Linux, Credential Manager on Windows).
- No plaintext credentials in config files.
- `akua whoami` returns the current identity, scopes, and registry logins.
- Tokens can be scoped per-registry; agents receive per-task scoped tokens that expire automatically.

---

## 9. Logging

- Default: human-readable text to stderr.
- `--log=json` — JSON-lines to stderr; auto-enabled in agent context.
- `--log-level=<debug|info|warn|error>` — filter; only applies to akua targets, transitive crates stay at `warn`.
- `-v` / `--verbose` — shorthand for `--log-level=debug`.
- Logs are separate from output. Output is the return value of the command; logs are observability.

Log lines under `--log=json` are JSON objects with `level`, `target`, `message`, `fields`, and an optional parent `span` block. The `target` field is dotted: `akua`, `akua::worker`, `akua::bridge`. Timestamps are omitted in JSON / agent mode so byte-deterministic golden tests can diff stderr; under text mode timestamps are included for human readability.

Structured errors (§1.2) remain a single terminal JSON object on stderr — distinguishable from log lines by the absence of a `level` field and the presence of a `code`.

`RUST_LOG` is the escape hatch. When set, it overrides the resolved filter directive entirely (full `EnvFilter` syntax). `AKUA_BRIDGE_TRACE=1` is a back-compat shortcut that ORs `akua::bridge=debug` into the filter.

### 9.1 OpenTelemetry export

OTLP export is enabled when `OTEL_EXPORTER_OTLP_ENDPOINT` (or `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`) is set in the environment. No CLI flag — the OTel spec defines a standard env-var surface and akua honors it directly:

- `OTEL_EXPORTER_OTLP_ENDPOINT` / `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`
- `OTEL_EXPORTER_OTLP_HEADERS`
- `OTEL_EXPORTER_OTLP_PROTOCOL` (gRPC over tonic; HTTP not yet wired)
- `OTEL_EXPORTER_OTLP_TIMEOUT`
- `OTEL_SERVICE_NAME`
- `OTEL_RESOURCE_ATTRIBUTES`

A single OTel trace covers `worker.invoke → worker.render_request → kcl eval`; bridge calls (`bridge.call`, `bridge.response`) appear as child events. Worker-side spans flow through the host's stderr-replay path so the entire pipeline shares one trace context.

Build the CLI without `--features otel` (off by default in the napi distribution) to drop the OTel + tokio runtime dependency; the rest of the logging contract is unaffected.

---

## 10. Stability

- Flags added after v1.0 are backward-compatible.
- Removing a flag requires deprecation cycle of at least 6 months.
- JSON output keys that appear in `--json` output are part of the stability contract.
- Exit codes never change meaning.
- Adding new exit codes is a breaking change requiring a major version bump.

---

## 11. No hidden state

- No ambient reads (environment variables, `~/.config/*`, `/etc/*`) unless the flag explicitly references them.
- Every input to a verb is discoverable via `--describe` or `--dry-run`.
- A verb's behavior is fully determined by its arguments + the files it operates on.

Exception: auth tokens from `akua login` are read from the credential store. This is the only ambient state.

---

## 12. Policy gating

Every write verb invokes the policy engine before acting. The result of the policy check is always present in output:

```json
{
  "policy": {
    "tier": "tier/production",
    "verdict": "allow" | "deny" | "needs-approval",
    "reason": "...",
    "suggested_fix": "...",
    "approvers": ["@team/platform"]
  }
}
```

When `verdict=deny`, the verb exits with code 3 and does not write.
When `verdict=needs-approval`, the verb exits with code 5 and does not write; it emits an approval URL.

---

## 13. Telemetry

- Off by default.
- Opt-in via `akua telemetry enable`.
- Anonymized, aggregated, documented.
- Includes exit codes, verb latencies, and feature usage. Never includes user data, file contents, or secrets.
- Can be audited: `akua telemetry show` prints the last 100 records that would have been sent.

---

## 14. What the contract is NOT

- Not a style guide. Individual verbs can choose between `get`/`describe`/`list` freely.
- Not a feature matrix. Verbs differ in what they do; they don't differ in how they behave.
- Not a Swagger replacement. `akua help --json` is for discovery; it's not an OpenAPI spec.

---

## 15. Enforcement

Every PR adding a verb or flag is reviewed against this contract. The CI lint step includes:

- `akua lint-cli` — checks every verb emits `--json`, has typed exit codes, accepts `--timeout`, passes `--describe --json` round-trip.
- Contract violations block merge.
- Contract amendments require RFC.

---

**This contract is the single thing that makes akua agent-friendly. The narrow verb surface, the typed exits, the structured JSON, the idempotency keys, the plan mode — each is a deliberate choice, each is load-bearing, each must hold for every verb.**

When in doubt: **obey the contract first, add the feature second.** A feature that requires violating the contract is either a missing primitive (add it generically to the contract) or not worth shipping.