/**
 * HTTP Helm repository pull — mirrors the `oci://` path in [`pullChart`]
 * for classic Helm HTTP repos (Bitnami, Jetstack, ArtifactHub-indexed
 * catalogs). No helm binary required.
 *
 * Flow:
 *   1. GET `<repo>/index.yaml`, parse via `yaml`.
 *   2. Find the `entries[chart][n]` where `version` matches.
 *   3. Resolve the chart URL (absolute or repo-relative).
 *   4. GET the `.tgz`, verify the index entry's `digest` (sha256).
 */

import { parse as parseYaml } from 'yaml';

import { AkuaError } from './errors.js';
import type { OciAuth, OciCredentials } from './oci.js';

export class HelmHttpError extends AkuaError {
  constructor(message: string, cause?: unknown) {
    super(message, cause);
    this.name = 'HelmHttpError';
  }
}

export interface HelmHttpPullOptions {
  /** Registry credentials keyed by host. Reuses [`OciAuth`] shape. */
  auth?: OciAuth;
  /** Hard cap on downloaded bytes (default 100 MB). */
  maxBytes?: number;
  /** Abort signal. */
  signal?: AbortSignal;
}

const DEFAULT_MAX_BYTES = 100 * 1024 * 1024;

interface HelmHttpRef {
  repo: string;
  chart: string;
  version: string;
}

/**
 * Parse `https://charts.example.com/ns/chart:1.2.3` into its parts. The
 * last path segment before `:<version>` is the chart name; everything
 * before it (including scheme) is the repo URL.
 */
export function parseHelmHttpRef(ref: string): HelmHttpRef {
  if (!ref.startsWith('https://') && !ref.startsWith('http://')) {
    throw new HelmHttpError(`not an http(s):// reference: ${ref}`);
  }
  const colonAt = ref.lastIndexOf(':');
  if (colonAt < 0 || colonAt < ref.indexOf('//')) {
    throw new HelmHttpError(`missing :<version> suffix in ${ref}`);
  }
  const pathPart = ref.slice(0, colonAt);
  const version = ref.slice(colonAt + 1);
  const slashAt = pathPart.lastIndexOf('/');
  if (slashAt < 0) {
    throw new HelmHttpError(`malformed repo path in ${ref}`);
  }
  const chart = pathPart.slice(slashAt + 1);
  const repo = pathPart.slice(0, slashAt);
  if (!chart || !version || !repo) {
    throw new HelmHttpError(`incomplete ref ${ref}`);
  }
  return { repo, chart, version };
}

/** Pull a chart from an HTTP(S) Helm repo. Returns raw tar+gzip bytes. */
export async function pullHelmHttpChart(
  ref: string,
  opts: HelmHttpPullOptions = {},
): Promise<Uint8Array> {
  const { repo, chart, version } = parseHelmHttpRef(ref);
  const maxBytes = opts.maxBytes ?? DEFAULT_MAX_BYTES;
  const host = new URL(repo).host;
  const authHeader = basicAuthHeader(opts.auth?.[host]);

  const indexUrl = `${repo.replace(/\/$/, '')}/index.yaml`;
  const indexResp = await fetch(indexUrl, {
    headers: authHeader ? { Authorization: authHeader } : {},
    signal: opts.signal,
  });
  if (!indexResp.ok) {
    throw new HelmHttpError(`index ${indexResp.status} ${indexResp.statusText} for ${indexUrl}`);
  }
  const indexText = await indexResp.text();
  const entry = findIndexEntry(indexText, chart, version);
  if (!entry) {
    throw new HelmHttpError(`chart ${chart}@${version} not found in ${indexUrl}`);
  }
  if (entry.urls.length === 0) {
    throw new HelmHttpError(`no urls for chart ${chart}@${version} in ${indexUrl}`);
  }

  const chartUrl = resolveChartUrl(repo, entry.urls[0]!);
  const chartResp = await fetch(chartUrl, {
    headers: authHeader ? { Authorization: authHeader } : {},
    signal: opts.signal,
  });
  if (!chartResp.ok) {
    throw new HelmHttpError(
      `chart ${chartResp.status} ${chartResp.statusText} for ${chartUrl}`,
    );
  }
  const buf = await chartResp.arrayBuffer();
  if (buf.byteLength > maxBytes) {
    throw new HelmHttpError(`received ${buf.byteLength} bytes, over limit ${maxBytes}`);
  }
  const bytes = new Uint8Array(buf);

  if (entry.digest) {
    await verifySha256(bytes, entry.digest, chartUrl);
  }
  return bytes;
}

function resolveChartUrl(repo: string, urlOrPath: string): string {
  if (urlOrPath.startsWith('http://') || urlOrPath.startsWith('https://')) {
    return urlOrPath;
  }
  return `${repo.replace(/\/$/, '')}/${urlOrPath.replace(/^\//, '')}`;
}

function basicAuthHeader(creds: OciCredentials | undefined): string | null {
  if (!creds) return null;
  if ('token' in creds) return `Bearer ${creds.token}`;
  const basic = typeof Buffer !== 'undefined'
    ? Buffer.from(`${creds.username}:${creds.password}`, 'utf8').toString('base64')
    : btoa(unescape(encodeURIComponent(`${creds.username}:${creds.password}`)));
  return `Basic ${basic}`;
}

async function verifySha256(bytes: Uint8Array, digest: string, source: string): Promise<void> {
  const hex = digest.startsWith('sha256:') ? digest.slice('sha256:'.length) : digest;
  const hash = await crypto.subtle.digest('SHA-256', bytes as BufferSource);
  const actual = Array.from(new Uint8Array(hash))
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('');
  if (actual !== hex.toLowerCase()) {
    throw new HelmHttpError(
      `digest mismatch for ${source}: index advertised ${hex}, got ${actual}`,
    );
  }
}

interface IndexEntry {
  version: string;
  urls: string[];
  digest: string | null;
}

interface ParsedIndex {
  entries?: Record<string, { version?: string; urls?: string[]; digest?: string }[]>;
}

/**
 * Extract the first matching `(chart, version)` entry from a Helm
 * `index.yaml`. Only reads `version`, `urls`, `digest` — everything
 * else in the entry is ignored.
 */
export function findIndexEntry(
  indexYaml: string,
  chart: string,
  version: string,
): IndexEntry | null {
  let parsed: ParsedIndex;
  try {
    parsed = parseYaml(indexYaml) as ParsedIndex;
  } catch (err) {
    throw new HelmHttpError(`failed to parse index.yaml: ${(err as Error).message}`, err);
  }
  const versions = parsed?.entries?.[chart];
  if (!versions || !Array.isArray(versions)) return null;
  const match = versions.find((e) => e.version === version);
  if (!match) return null;
  return {
    version,
    urls: Array.isArray(match.urls) ? match.urls.map(String) : [],
    digest: match.digest ? String(match.digest) : null,
  };
}
