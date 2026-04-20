# Security

akua's threat model assumes the **package author is untrusted**. A
worker process, CI runner, local developer, or agent sandbox should be
able to run `akua render` / `akua inspect` / `akua publish` / `akua
policy check` / `@akua/sdk` calls against attacker-controlled
`Package.k` + chart tarballs + Rego modules **without an OS-level
sandbox** for the vast majority of workflows.

This doc lists the hardening that's in place and the remaining caveats
— the one engine (helmfile) that still requires a sandbox, plus known
limitations we're tracking.

## Fixed attack surfaces

### Tar extraction (P0 — fixed)

`unpack_chart_tgz` (CLI/Rust) rejects any non-regular-file, non-dir tar
entry — symlinks, hard links, device files, FIFOs all error out with
`UnsafeEntryPath`. Path components are also checked: absolute paths,
`..`, and Windows prefixes are rejected. The SDK's
`streamTgzEntries` filters on `typeflag` and only yields regular files.

Without this, a malicious `mychart.tgz` could ship
`mychart/values.schema.json -> /etc/passwd` and a subsequent
`akua inspect` would read the symlink target and surface its contents
in the JSON output.

### Decompression bombs + entry-count caps

Three env-configurable limits:
- `AKUA_MAX_DOWNLOAD_BYTES` (100 MB default) — caps network download.
- `AKUA_MAX_EXTRACTED_BYTES` (500 MB) — caps gunzip output.
- `AKUA_MAX_TAR_ENTRIES` (20 000) — caps per-archive entry count.

All three are enforced in the streaming path — no part of a bomb ever
lands fully in memory or on disk.

### Path traversal (tar)

`validate_tar_entry_path` rejects absolute paths, `..` components, and
Windows root/prefix components before the entry is unpacked.

### SSRF (P1 — fixed)

Repo URLs resolving to private / loopback / link-local IP literals are
rejected:
- `127.0.0.0/8`, `169.254.0.0/16` (AWS metadata), RFC1918
  (`10/8`, `172.16/12`, `192.168/16`), CGNAT (`100.64/10`),
  `0.0.0.0`, `255.255.255.255`
- IPv6: `::1`, `fc00::/7`, `fe80::/10`

Applies to `oci://` and `http(s)://` repos. Redirect responses are
re-validated — a public registry can't 302 akua to the cloud metadata
endpoint. Both the Rust fetch path and the SDK's `pullChart` enforce
this. Set `AKUA_ALLOW_PRIVATE_HOSTS=1` to bypass for local dev.

**Known limitation**: DNS names resolving to private IPs aren't caught
(DNS rebinding). Mitigate at the network layer — egress firewall
rules are the proper control.

### KCL evaluation (Package authoring)

Packages are authored in KCL. The embedded KCL interpreter (via
`kclvm-rs`) runs **without** filesystem, network, process, or env
access — KCL is a pure functional language by design, and akua does
not register host functions that break that. Every `Package.k` compiles
to a pure function of its inputs. No RCE surface from malicious KCL.

### CEL expression execution (input transforms / policy)

CEL runs via `cel-interpreter`. The interpreter **does not** expose
filesystem, network, process, or environment builtins. The only custom
functions akua registers are `slugify` / `slugifyMax` — pure. There's
no RCE vector from a malicious CEL expression, only DoS via
long-running expressions (see below).

### Rego evaluation (Policies)

Rego runs via embedded OPA. OPA's evaluator has no host-access
builtins by default, and akua does not register side-effecting
functions. Policies evaluate in-process, offline, deterministically.

### Embedded engines (Helm, Kustomize, kro, Kyverno→Rego)

Engines run inside wasmtime with **no WASI filesystem capabilities**.
Template expressions in a malicious chart / overlay / RGD can't escape
the WASM sandbox. See [`embedded-engines.md`](./docs/embedded-engines.md)
for the full list and the embedding contract.

### OCI manifest auth leakage

`OciAuth` / `RegistryCredentials` are not logged. A custom `Debug`
impl redacts passwords and bearer tokens (fixed in 0.3.0).

## Remaining caveats

### Helmfile engine (OFF by default)

`engine-helmfile` is **disabled in the default cargo feature set** as
of 0.3.0. Helmfile's Go-template layer evaluates `exec`,
`requiredEnv`, `readFile`, and other side-effecting functions — a
malicious `helmfile.yaml` achieves arbitrary command execution at
build time.

Enable only if you trust every package you build:

```sh
cargo build -p akua-cli --features akua-core/engine-helmfile
```

When on, akua still validates source paths but cannot constrain what
helmfile itself does — a sandbox is the only real mitigation.

### CEL / Rego DoS (P2 — documented)

A malicious CEL or Rego expression can hang the worker thread (runaway
comprehensions, long string concatenation up to RAM, exponential
partial evaluation). Bound at the caller with a wall-clock budget or
`AbortSignal` — akua does not yet enforce a per-call timeout
internally. Every verb accepts `--timeout`; the CLI dispatches that to
the engine. Tracked for a future release where the internal timer is
authoritative.

### `serde_yaml` deprecated upstream

`serde_yaml` (the Rust YAML parser) is unmaintained. Migration to
`serde-yml` is tracked. No known exploit today; worst case is a YAML
bomb consuming up to `AKUA_MAX_DOWNLOAD_BYTES` of allocator memory.

### `--engine=shell` bypasses akua's controls

`--engine=shell` forwards rendering to the engine's CLI binary on the
user's `$PATH`, which then performs its own dep fetch using its own
config / redirect / auth rules. If you're feeding akua untrusted
packages, stay on the default `--engine=embedded`. Shell-out is
retained as an escape hatch; it's safe only for trusted packages.

## Reporting

Please report vulnerabilities privately via GitHub Security Advisories
at https://github.com/cnap-tech/akua/security/advisories/new.
Fixes are prioritized ahead of feature work; we'll coordinate
disclosure timing with you.
