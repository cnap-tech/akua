---
name: rotate-secret
description: Rotate a shared secret across every install that references it. Use when a credential is compromised, during scheduled key rotation (e.g., 30-day policy), when an API token expires, or when adopting a new signing key across a fleet. Handles fan-out to all consumers, policy-gated rollout, and audit trail.
license: Apache-2.0
---

# Rotate a shared secret across installs

Secrets in akua are refs (`vault://…`, `infisical://…`, etc.) — the consuming resource references the secret by name, never by value. Rotation generates a new value in the store, updates the reference, and triggers a coordinated re-deploy of everything that reads it.

## When to use

- Scheduled rotation per your secret's `rotation.policy` (30d, 90d, etc.)
- Emergency rotation after a suspected leak
- Revoking access for a departed engineer (indirect — rotate secrets they had access to)
- Annual cryptographic key rotation for signing keys, TLS certs, etc.

## Steps

### 1. Plan the rotation

Preview impact before acting:

```sh
akua secret trace stripe-api-key --json
```

Returns who has the secret granted, which services reference it, last-access timestamps. Example:

```json
{
  "name": "stripe-api-key",
  "store": "vault",
  "ref": "vault://secrets/stripe/api-key",
  "grants": [
    {"service": "checkout", "scope": "read", "granted_at": "2026-01-15"},
    {"service": "webhook-handler", "scope": "read", "granted_at": "2026-02-03"}
  ],
  "last_access": "2026-04-20T14:03:00Z"
}
```

Note which services will need to roll. Confirm you have approval from the on-call for each.

### 2. Dry-run the rotation

```sh
akua secret rotate stripe-api-key --plan --json
```

Plan output includes: new secret version, affected services, policy verdict (likely needs-approval for production-tier secrets), estimated rollout duration.

### 3. Execute with an idempotency key

```sh
IDEMP=$(uuidgen)
akua secret rotate stripe-api-key --idempotency-key=$IDEMP
```

The idempotency key means you can retry safely if the call fails mid-flight — the second call returns the same new-version record without generating a second rotation.

The command:

- Generates a new secret value in the backing store
- Updates the secret ref metadata (new version, `last_rotated` timestamp)
- Publishes a `SecretRotated` event on the audit spine
- Does NOT yet trigger consumer re-deploy — that's the next step

### 4. Roll consumers

`akua secret rotate` emits the secret version bump; consuming services must re-read the new value. For services that read secrets at startup, this means a pod restart:

```sh
akua rollout apply --for-secret=stripe-api-key --strategy=staged --batch-size=2 --soak=2m
```

Staged rollout:

- Rolls 2 consumers at a time
- Soaks for 2 minutes between batches
- Checks health gate (configured in the workspace's environment schema) after each batch
- Aborts + auto-rollback on health regression

For services that hot-reload secrets (via External Secrets Operator + SIGHUP, or Vault Agent), no pod restart is needed — the operator handles it. akua detects this via the consumer's `kind: App` annotation.

### 5. Verify

```sh
akua secret trace stripe-api-key --json
akua query "error_rate p99 last 30m" --json
```

Check: `last_rotated` is now. Error rate across consumers has not spiked. Audit spine shows the rotation actor + timestamp.

### 6. Rollback (if needed)

If a consumer breaks after rotation:

```sh
akua secret rotate stripe-api-key --rollback-to=<previous-version>
akua rollout apply --for-secret=stripe-api-key --strategy=staged
```

The previous version is retained in the backing store for N days (configured per secret). After retention, rollback is no longer possible — plan accordingly.

## Agent-friendly variant

Agent drivers should:

1. Always dry-run with `--plan --json` first.
2. Parse the plan output and confirm no `policy.verdict` is `deny`.
3. If `needs-approval`, post the approval URL to the human and wait.
4. Execute with a fresh UUID per logical operation.
5. Roll consumers with `--strategy=staged`; agents don't do big-bang changes.

## Failure modes

- **`E_SECRET_STORE_UNREACHABLE`** — backing store (Vault, Infisical) is down. Retry with same idempotency key.
- **`E_POLICY_DENY`** (exit 3) — rotation requires approvals you don't have. Check `akua policy check --for-secret=<name>`.
- **Consumer re-deploy fails** — rollout auto-rollbacks; secret rotation is still committed. Investigate the failing consumer; it likely has a stale reference or hardcoded old value.
- **New version not readable by consumer** — grant scoping changed during rotation. `akua secret grant <name> --to=<service> --scope=read` to re-grant.

## Reference

- [cli.md — akua secret](../../docs/cli.md#akua-secret)
- [cli.md — akua rollout](../../docs/cli.md#akua-rollout)
- Policy tier reference for secret rotation rules: `docs/policies/`
