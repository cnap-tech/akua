/**
 * Shared credential helpers used by both the OCI and HTTP Helm pull
 * paths. Kept in its own module so `helm-http.ts`, `oci.ts`, and
 * `docker-config.node.ts` don't maintain parallel copies.
 */

import type { OciCredentials } from './oci.js';

/**
 * Translate [`OciCredentials`] into an HTTP `Authorization` header
 * value. `{ token }` → `Bearer …`; `{ username, password }` →
 * `Basic <base64>`. Returns `null` when `creds` is undefined.
 */
export function credentialsToAuthHeader(creds: OciCredentials | undefined): string | null {
  if (!creds) return null;
  if ('token' in creds) return `Bearer ${creds.token}`;
  return `Basic ${base64Encode(`${creds.username}:${creds.password}`)}`;
}

/** Base64-encode a UTF-8 string. Works in Node (Buffer) and browsers (btoa). */
export function base64Encode(s: string): string {
  if (typeof Buffer !== 'undefined') {
    return Buffer.from(s, 'utf8').toString('base64');
  }
  return btoa(unescape(encodeURIComponent(s)));
}

/** Normalise a Docker `auths` server address to a bare host. */
export function toHost(serverOrUrl: string): string {
  try {
    const u = new URL(serverOrUrl.includes('://') ? serverOrUrl : `https://${serverOrUrl}`);
    return u.host;
  } catch {
    return serverOrUrl;
  }
}
