/**
 * SSRF guard — reject repo URLs whose host is a private / loopback /
 * link-local IP literal. Mirrors `akua-core::ssrf::validate_host`.
 *
 * Limitation: DNS names resolving to private IPs aren't caught here
 * (DNS rebinding is a network-layer problem). Bypass entirely with
 * `AKUA_ALLOW_PRIVATE_HOSTS=1`.
 */

import { AkuaError } from './errors.js';

export class SsrfError extends AkuaError {
  constructor(host: string) {
    super(
      `refusing to fetch from private-range host \`${host}\`. Set AKUA_ALLOW_PRIVATE_HOSTS=1 for local dev.`,
    );
    this.name = 'SsrfError';
  }
}

export function validateHost(host: string): void {
  if (allowPrivate()) return;
  const bare = stripPort(host);
  if (isIpLiteral(bare) && isPrivate(bare)) {
    throw new SsrfError(host);
  }
}

/** Pull out the `host:port` from a full URL and validate. */
export function validateUrl(url: string): void {
  try {
    const u = new URL(url);
    validateHost(u.host);
  } catch (err) {
    if (err instanceof SsrfError) throw err;
    // Malformed URL — let the caller's fetch surface the error.
  }
}

function allowPrivate(): boolean {
  const proc = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process;
  const v = proc?.env?.AKUA_ALLOW_PRIVATE_HOSTS;
  return v === '1' || v === 'true' || v === 'yes';
}

function stripPort(host: string): string {
  // IPv6 bracketed literal: `[::1]:8080`
  if (host.startsWith('[')) {
    const end = host.indexOf(']');
    return end > 0 ? host.slice(1, end) : host;
  }
  const colon = host.lastIndexOf(':');
  if (colon < 0) return host;
  const maybePort = host.slice(colon + 1);
  if (/^\d+$/.test(maybePort)) return host.slice(0, colon);
  return host;
}

function isIpLiteral(host: string): boolean {
  return /^\d+\.\d+\.\d+\.\d+$/.test(host) || host.includes(':');
}

function isPrivate(host: string): boolean {
  // IPv4
  const v4 = host.match(/^(\d+)\.(\d+)\.(\d+)\.(\d+)$/);
  if (v4) {
    const [, a, b] = v4.map(Number) as [number, number, number, number, number];
    if (a === 127) return true; // loopback
    if (a === 10) return true; // RFC1918
    if (a === 172 && b >= 16 && b <= 31) return true;
    if (a === 192 && b === 168) return true;
    if (a === 169 && b === 254) return true; // link-local (AWS metadata)
    if (a === 100 && b >= 64 && b <= 127) return true; // CGNAT
    if (a === 0) return true; // unspecified
    if (a === 255 && b === 255) return true; // broadcast
    return false;
  }
  // IPv6 — crude but covers the common cases. `::1`, `fc00::/7`, `fe80::/10`.
  if (host.includes(':')) {
    const norm = host.toLowerCase();
    if (norm === '::1' || norm === '::') return true;
    // fc00::/7 → first byte 0xfc or 0xfd
    if (/^fc[0-9a-f]{0,2}:/.test(norm) || /^fd[0-9a-f]{0,2}:/.test(norm)) return true;
    // fe80::/10 → fe80-febf
    if (/^fe[89ab][0-9a-f]?:/.test(norm)) return true;
  }
  return false;
}
