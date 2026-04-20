---
name: dev-loop
description: Run a sub-second hot-reload development loop against a local Kubernetes cluster using `akua dev`. Use when iterating on a Package, debugging rendering, seeing a live diff of manifests as schema or inputs change, demoing an infra change, or when a user asks to preview how a change will affect deployed resources.
license: Apache-2.0
compatibility: Requires Docker (for kind/k3d), or an existing k8s cluster context. Port 5173 available for the browser UI.
---

# Hot-reload development with `akua dev`

`akua dev` is the signature akua experience. Watches the workspace; renders on every file save; applies to a local cluster in under 500ms; surfaces pipeline events in a browser UI at `http://localhost:5173`.

## When to use

- Iterating on a Package's schema or inputs
- Debugging why a chart renders incorrectly
- Verifying that a change doesn't break existing manifests before opening a PR
- Demoing a package to teammates in real time
- Exploring what a package does without deploying to a shared cluster

## Steps

### 1. Start the loop

From the workspace root:

```sh
akua dev
```

On first run: creates a kind cluster named `akua-dev`, installs Traefik ingress, sets up `*.127.0.0.1.nip.io` DNS. Browser opens `http://localhost:5173`.

Output shows watched workspace, target cluster, and UI URL:

```
✓ workspace: 3 apps, 2 envs, 1 policy tier
✓ target: local (kind cluster "akua-dev")
✓ ui: http://localhost:5173 opened
```

### 2. Edit and observe

Edit any file under `apps/`, `environments/`, `policies/`, or source engine files. Save.

The pipeline fires: parse → validate → render → policy check → diff → apply → reconcile. Each stage streams an event to the UI and a log line to stdout:

```
[edit apps/api/inputs.yaml: replicas 1 → 3]
  parsed       11ms
  validated    18ms  ✓ schema
  rendered    127ms  ✓ 4 manifests (1 changed)
  policy        9ms  ✓ tier/startup allows
  diff         41ms
  applied     281ms  ✓ patched deployment/api
  reconciled  1.1s   ✓ 3/3 replicas ready
  ↻ hot                (edit→steady = 1.4s)
```

### 3. Agent-operated mode

If an agent is driving the dev loop, use `--json` (auto-enabled when agent context detected — see [CLI contract §1.5](../../docs/cli-contract.md#15-agent-context-auto-detection)):

```sh
akua dev --json
```

Each line is a JSON event:

```json
{"t":1713636000,"stage":"render","app":"api","duration_ms":127,"status":"ok"}
{"t":1713636001,"stage":"apply","resource":"Deployment/api","op":"patch","duration_ms":198}
{"t":1713636002,"stage":"reconcile","resource":"Deployment/api","status":"ready"}
```

### 4. Useful flags

- `--target=dry-run` — render and validate without applying; still fast feedback
- `--target=cluster:<name>` — use a specific kubeconfig context (not kind)
- `--policy=<tier>` — apply production-tier policy locally to preview what prod would allow
- `--fresh` — wipe persistent state before starting (clears Postgres volumes, secrets, cached images)
- `--inputs=<file>` — override the default inputs file

### 5. Stop cleanly

Ctrl-C. The process:

- Drains in-flight reconciliations gracefully (up to `--shutdown-timeout`, default 10s)
- Preserves the kind cluster + persistent data (next `akua dev` resumes where you left off)
- Closes the browser UI

To fully reset:

```sh
akua dev --fresh
# OR
kind delete cluster --name akua-dev
```

## Interpreting events

- **`render` slow (>500ms)** — workspace too large, or a source engine is misbehaving. Profile with `--log-level=debug`.
- **`policy` denies** — UI shows the failing rule and the field/resource at fault. Fix the input and save; re-check is automatic.
- **`reconcile` stuck** — pod crashlooping or health-check failing. UI surfaces last log lines; follow links to `kubectl describe`.
- **`drift detected`** — someone ran `kubectl apply` outside `akua dev`. Options: `adopt` (accept cluster state as new desired) or `revert` (snap cluster back to desired).

## Failure modes

- **Docker not running** — `akua dev` needs Docker for kind. Start Docker Desktop / colima / podman.
- **Port 5173 in use** — `akua dev --ui-port 5174`
- **kubeconfig not found** — set `$KUBECONFIG` or use `--target=cluster:<name>`
- **Persistent data corrupt after abrupt shutdown** — `akua dev --fresh` wipes and restarts

## Reference

- [cli.md — akua dev](../../docs/cli.md#akua-dev)
- [Masterplan §11 — the signature experience](https://github.com/cnap-tech/cortex/blob/docs/cnap-masterplan/workspaces/robin/akua-masterplan.md)
