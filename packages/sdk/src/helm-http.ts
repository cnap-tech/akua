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

import { credentialsToAuthHeader } from './auth.js';
import { AkuaError } from './errors.js';
import type { OciAuth } from './oci.js';
import { validateHost } from './ssrf.js';

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

/**
 * Per-repo in-flight promise cache for `index.yaml` fetches. A build
 * pulling N dependencies from the same repo incurs exactly one network
 * round-trip for the index. Exposed via `clearIndexCache` in tests.
 */
const INDEX_CACHE = new Map<string, Promise<string>>();

export function clearIndexCache(): void {
  INDEX_CACHE.clear();
}

/** Pull a chart from an HTTP(S) Helm repo. Returns raw tar+gzip bytes. */
export async function pullHelmHttpChart(
  ref: string,
  opts: HelmHttpPullOptions = {},
): Promise<Uint8Array> {
  const { bytes } = await fetchHelmHttpChart(ref, opts);
  return bytes;
}

/**
 * Streaming variant. Returns the chart `.tgz` as a
 * `ReadableStream<Uint8Array>`. Digest verification is **not** performed
 * in the streaming path — the bytes pass straight through to the
 * consumer. Use [`pullHelmHttpChart`] when you need digest verification.
 */
export async function pullHelmHttpChartStream(
  ref: string,
  opts: HelmHttpPullOptions = {},
): Promise<ReadableStream<Uint8Array>> {
  const { resp } = await resolveHelmHttpChart(ref, opts);
  if (!resp.body) {
    throw new HelmHttpError(`chart response has no body for ${ref}`);
  }
  return resp.body;
}

interface ResolvedHelmHttp {
  resp: Response;
  chartUrl: string;
  digest: string | null;
}

async function resolveHelmHttpChart(
  ref: string,
  opts: HelmHttpPullOptions,
): Promise<ResolvedHelmHttp> {
  const { repo, chart, version } = parseHelmHttpRef(ref);
  const maxBytes = opts.maxBytes ?? DEFAULT_MAX_BYTES;
  const host = new URL(repo).host;
  validateHost(host);
  const authHeader = credentialsToAuthHeader(opts.auth?.[host]);

  const indexText = await getIndex(repo, authHeader, opts.signal);
  const entry = findIndexEntry(indexText, chart, version);
  if (!entry) {
    throw new HelmHttpError(`chart ${chart}@${version} not found in ${repo}/index.yaml`);
  }
  if (entry.urls.length === 0) {
    throw new HelmHttpError(`no urls for chart ${chart}@${version} in ${repo}/index.yaml`);
  }

  const chartUrl = resolveChartUrl(repo, entry.urls[0]!);
  const resp = await fetch(chartUrl, {
    headers: authHeader ? { Authorization: authHeader } : {},
    signal: opts.signal,
  });
  if (!resp.ok) {
    throw new HelmHttpError(`chart ${resp.status} ${resp.statusText} for ${chartUrl}`);
  }
  // Preflight against Content-Length so servers without enforcement
  // can't OOM the client with a gigabyte response.
  const advertised = Number(resp.headers.get('content-length') ?? '');
  if (Number.isFinite(advertised) && advertised > maxBytes) {
    throw new HelmHttpError(
      `advertised size ${advertised} > limit ${maxBytes} — override with maxBytes`,
    );
  }
  return { resp, chartUrl, digest: entry.digest };
}

async function fetchHelmHttpChart(
  ref: string,
  opts: HelmHttpPullOptions,
): Promise<{ bytes: Uint8Array }> {
  const { resp, chartUrl, digest } = await resolveHelmHttpChart(ref, opts);
  const maxBytes = opts.maxBytes ?? DEFAULT_MAX_BYTES;
  if (!resp.body) {
    throw new HelmHttpError(`chart response has no body for ${chartUrl}`);
  }
  // Stream-consume with a running byte cap — a server that lies about
  // Content-Length or omits it entirely can't OOM us by sending more
  // than we asked for.
  const reader = resp.body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      total += value.byteLength;
      if (total > maxBytes) {
        await reader.cancel(`over limit ${maxBytes}`).catch(() => {});
        throw new HelmHttpError(`received ${total} bytes, over limit ${maxBytes}`);
      }
      chunks.push(value);
    }
  } finally {
    reader.releaseLock();
  }
  const bytes = new Uint8Array(total);
  let offset = 0;
  for (const c of chunks) {
    bytes.set(c, offset);
    offset += c.byteLength;
  }
  if (digest) {
    await verifySha256(bytes, digest, chartUrl);
  }
  return { bytes };
}

async function getIndex(
  repo: string,
  authHeader: string | null,
  signal: AbortSignal | undefined,
): Promise<string> {
  const key = repo;
  const cached = INDEX_CACHE.get(key);
  if (cached) return cached;
  const indexUrl = `${repo.replace(/\/$/, '')}/index.yaml`;
  const fetchPromise = fetch(indexUrl, {
    headers: authHeader ? { Authorization: authHeader } : {},
    signal,
  }).then(async (resp) => {
    if (!resp.ok) {
      throw new HelmHttpError(`index ${resp.status} ${resp.statusText} for ${indexUrl}`);
    }
    return resp.text();
  });
  // Cache the promise (not the resolved text) so concurrent callers
  // share the one in-flight fetch. Evict on failure so we don't
  // permanently remember a transient error.
  const wrapped = fetchPromise.catch((err) => {
    INDEX_CACHE.delete(key);
    throw err;
  });
  INDEX_CACHE.set(key, wrapped);
  return wrapped;
}

function resolveChartUrl(repo: string, urlOrPath: string): string {
  if (urlOrPath.startsWith('http://') || urlOrPath.startsWith('https://')) {
    return urlOrPath;
  }
  return `${repo.replace(/\/$/, '')}/${urlOrPath.replace(/^\//, '')}`;
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
