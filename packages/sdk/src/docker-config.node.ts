/**
 * Node-only helper that translates a Docker client config
 * (`~/.docker/config.json`) into the SDK's [`OciAuth`] shape.
 *
 * Handles:
 *   - `auths["host"].auth` — base64(`user:password`)
 *   - `auths["host"].username` / `.password` — literal pair
 *   - `auths["host"].identitytoken` — OAuth refresh token (Bearer)
 *   - `credHelpers["host"]` / `credsStore` — invokes `docker-credential-*`
 *     binaries on PATH and parses their `{ "Username": "...", "Secret": "..." }`
 *     JSON response.
 *
 * Not exposed from `@akua/sdk/browser` — filesystem + `child_process`
 * aren't available there.
 */

import { readFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';

import { toHost } from './auth.js';
import { AkuaError } from './errors.js';
import type { OciAuth, OciCredentials } from './oci.js';

export class DockerConfigError extends AkuaError {
  constructor(message: string, cause?: unknown) {
    super(message, cause);
    this.name = 'DockerConfigError';
  }
}

export interface DockerConfigAuthOptions {
  /** Override path. Defaults to `$DOCKER_CONFIG/config.json` or `~/.docker/config.json`. */
  path?: string;
  /**
   * Restrict to specific hosts — skips credential-helper lookups for
   * other hosts (which can be expensive / prompt for user). Omit to
   * resolve every host present in the config.
   *
   * Note: `credsStore` (if configured) is only invoked when `hosts` is
   * set. Without `hosts` the SDK has no list of hosts to query the
   * store for — consumers are expected to pass `hosts` when they
   * depend on a store like `osxkeychain`.
   */
  hosts?: string[];
  /**
   * Per-helper timeout in ms (default 5000). Prevents a hung keychain
   * prompt (e.g. `docker-credential-osxkeychain`) from stalling the
   * whole pipeline. The helper process is killed on timeout.
   */
  helperTimeoutMs?: number;
}

interface DockerConfigFile {
  auths?: Record<
    string,
    { auth?: string; username?: string; password?: string; identitytoken?: string }
  >;
  credHelpers?: Record<string, string>;
  credsStore?: string;
}

/**
 * Read a Docker client config and translate it into [`OciAuth`]. Pair
 * with `pullChart({ auth: await dockerConfigAuth() })`.
 */
export async function dockerConfigAuth(
  options: DockerConfigAuthOptions = {},
): Promise<OciAuth> {
  const path = options.path ?? resolveConfigPath();
  let config: DockerConfigFile;
  try {
    const raw = await readFile(path, 'utf8');
    config = JSON.parse(raw) as DockerConfigFile;
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === 'ENOENT') return {};
    throw new DockerConfigError(`failed to read docker config at ${path}`, err);
  }

  const wanted = options.hosts ? new Set(options.hosts) : null;
  const timeoutMs = options.helperTimeoutMs ?? 5000;
  const out: OciAuth = {};

  for (const [serverAddress, entry] of Object.entries(config.auths ?? {})) {
    const host = toHost(serverAddress);
    if (wanted && !wanted.has(host)) continue;
    const creds = credsFromAuthEntry(entry);
    if (creds) out[host] = creds;
  }

  const helperJobs: Promise<void>[] = [];
  for (const [serverAddress, helper] of Object.entries(config.credHelpers ?? {})) {
    const host = toHost(serverAddress);
    if (wanted && !wanted.has(host)) continue;
    if (out[host]) continue;
    helperJobs.push(
      runCredHelper(helper, serverAddress, timeoutMs).then((creds) => {
        if (creds) out[host] = creds;
      }),
    );
  }
  // `credsStore` is a global fallback — but resolving it requires a
  // per-host query. Without a `hosts` filter there's nothing to query
  // for, so the store is skipped. Consumers depending on a store pass
  // `hosts: [...]` so we know which to ask for.
  if (config.credsStore && wanted) {
    for (const host of wanted) {
      if (out[host]) continue;
      helperJobs.push(
        runCredHelper(config.credsStore, host, timeoutMs).then((creds) => {
          if (creds) out[host] = creds;
        }),
      );
    }
  }
  await Promise.all(helperJobs);
  return out;
}

function resolveConfigPath(): string {
  const envBase = process.env.DOCKER_CONFIG;
  if (envBase) return join(envBase, 'config.json');
  return join(homedir(), '.docker', 'config.json');
}

function credsFromAuthEntry(
  entry: { auth?: string; username?: string; password?: string; identitytoken?: string },
): OciCredentials | null {
  if (entry.identitytoken) return { token: entry.identitytoken };
  if (entry.auth) {
    const decoded = Buffer.from(entry.auth, 'base64').toString('utf8');
    const idx = decoded.indexOf(':');
    if (idx > 0) {
      return { username: decoded.slice(0, idx), password: decoded.slice(idx + 1) };
    }
  }
  if (entry.username && entry.password) {
    return { username: entry.username, password: entry.password };
  }
  return null;
}

interface CredHelperResponse {
  Username?: string;
  Secret?: string;
}

/**
 * Reject `helper` values that could cause the spawn to resolve an
 * unexpected binary (path separators) or escape into shell metachars.
 * Docker's spec says the name is a bare identifier.
 */
function isValidHelperName(helper: string): boolean {
  return /^[A-Za-z0-9._-]+$/.test(helper);
}

async function runCredHelper(
  helper: string,
  host: string,
  timeoutMs: number,
): Promise<OciCredentials | null> {
  if (!isValidHelperName(helper)) return null;
  const binary = `docker-credential-${helper}`;
  return new Promise<OciCredentials | null>((resolve) => {
    const child = spawn(binary, ['get'], { stdio: ['pipe', 'pipe', 'pipe'] });
    const out: Buffer[] = [];
    let settled = false;
    const finish = (creds: OciCredentials | null): void => {
      if (settled) return;
      settled = true;
      resolve(creds);
    };

    // Drain stderr to prevent the pipe buffer from filling (which would
    // block the helper process on write).
    child.stderr.on('data', () => {});
    child.stdout.on('data', (d: Buffer) => out.push(d));
    child.on('error', () => finish(null));

    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      finish(null);
    }, timeoutMs);
    timer.unref?.();

    child.on('close', (code) => {
      clearTimeout(timer);
      if (code !== 0) return finish(null);
      try {
        const body = JSON.parse(Buffer.concat(out).toString('utf8')) as CredHelperResponse;
        if (!body.Username || !body.Secret) return finish(null);
        // Per Docker spec, `<token>` username means Secret is an identity token.
        if (body.Username === '<token>') {
          finish({ token: body.Secret });
        } else {
          finish({ username: body.Username, password: body.Secret });
        }
      } catch {
        finish(null);
      }
    });
    child.stdin.end(host);
  });
}
