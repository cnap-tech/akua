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
   */
  hosts?: string[];
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
  const out: OciAuth = {};

  for (const [serverAddress, entry] of Object.entries(config.auths ?? {})) {
    const host = hostFromServer(serverAddress);
    if (wanted && !wanted.has(host)) continue;
    const creds = credsFromAuthEntry(entry);
    if (creds) out[host] = creds;
  }

  const helperJobs: Promise<void>[] = [];
  for (const [serverAddress, helper] of Object.entries(config.credHelpers ?? {})) {
    const host = hostFromServer(serverAddress);
    if (wanted && !wanted.has(host)) continue;
    if (out[host]) continue;
    helperJobs.push(
      runCredHelper(helper, serverAddress).then((creds) => {
        if (creds) out[host] = creds;
      }),
    );
  }
  if (config.credsStore && wanted) {
    for (const host of wanted) {
      if (out[host]) continue;
      helperJobs.push(
        runCredHelper(config.credsStore, host).then((creds) => {
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

/**
 * Strip scheme + path from a Docker `auths` key. Docker often writes
 * `https://index.docker.io/v1/` as the server address; consumers
 * dial `registry-1.docker.io` without scheme. Normalise to host.
 */
function hostFromServer(serverAddress: string): string {
  try {
    const u = new URL(
      serverAddress.includes('://') ? serverAddress : `https://${serverAddress}`,
    );
    return u.host;
  } catch {
    return serverAddress;
  }
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

async function runCredHelper(helper: string, host: string): Promise<OciCredentials | null> {
  const binary = `docker-credential-${helper}`;
  return new Promise<OciCredentials | null>((resolve) => {
    const child = spawn(binary, ['get'], { stdio: ['pipe', 'pipe', 'pipe'] });
    const out: Buffer[] = [];
    child.stdout.on('data', (d: Buffer) => out.push(d));
    child.on('error', () => resolve(null));
    child.on('close', (code) => {
      if (code !== 0) return resolve(null);
      try {
        const body = JSON.parse(Buffer.concat(out).toString('utf8')) as CredHelperResponse;
        if (!body.Username || !body.Secret) return resolve(null);
        // Per Docker spec, <token> username means Secret is an identity token.
        if (body.Username === '<token>') {
          resolve({ token: body.Secret });
        } else {
          resolve({ username: body.Username, password: body.Secret });
        }
      } catch {
        resolve(null);
      }
    });
    child.stdin.end(host);
  });
}
