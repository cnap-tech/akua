/**
 * OCI chart pull — pure TypeScript, no helm binary required.
 *
 * Ports the logic in `akua-core::fetch::fetch_oci`:
 *  1. HEAD the manifest URL; on `401 Bearer realm=…,service=…,scope=…`,
 *     exchange for a read-only token (the ghcr.io public-chart dance
 *     that oci-client's default probe misses).
 *  2. Pull the manifest, check advertised layer size against the limit.
 *  3. Pull the single chart layer (tar+gzip).
 *  4. Return the bytes.
 *
 * Primary target is Node; should also work in browsers for public CORS-
 * enabled registries (most don't emit CORS headers, so browsers will
 * typically only consume bytes fetched elsewhere).
 */

export type OciCredentials =
  | { username: string; password: string }
  | { token: string };

/** Caller-supplied credentials keyed by registry host. */
export type OciAuth = Record<string, OciCredentials>;

export interface PullChartOptions {
  /** Per-host registry credentials. Falls back to anonymous on miss. */
  auth?: OciAuth;
  /**
   * Hard cap on downloaded bytes. Default 100 MB, matching the CLI's
   * `AKUA_MAX_DOWNLOAD_BYTES`. Set higher only if you trust the source.
   */
  maxBytes?: number;
  /** Abort signal — aborts in-flight fetches when cancelled. */
  signal?: AbortSignal;
}

const DEFAULT_MAX_BYTES = 100 * 1024 * 1024;

const HELM_LAYER_MEDIA_TYPE = 'application/vnd.cncf.helm.chart.content.v1.tar+gzip';
const MANIFEST_ACCEPT = [
  'application/vnd.oci.image.manifest.v1+json',
  'application/vnd.docker.distribution.manifest.v2+json',
].join(', ');

export class OciPullError extends Error {
  constructor(message: string, readonly cause_?: unknown) {
    super(message);
    this.name = 'OciPullError';
  }
}

/**
 * Parse `oci://host/path/chart:version` into its registry-path parts.
 * Exported for tests; callers should use `pullChart`.
 */
export function parseOciRef(ref: string): { host: string; repository: string; tag: string } {
  if (!ref.startsWith('oci://')) {
    throw new OciPullError(`not an oci:// reference: ${ref}`);
  }
  const without = ref.slice('oci://'.length);
  const [pathPart, tag] = splitLast(without, ':');
  if (!tag) {
    throw new OciPullError(`missing :<version> suffix in ${ref}`);
  }
  const [host, ...rest] = pathPart.split('/');
  if (!host || rest.length === 0) {
    throw new OciPullError(`missing chart path in ${ref}`);
  }
  return { host, repository: rest.filter(Boolean).join('/'), tag };
}

function splitLast(s: string, sep: string): [string, string | undefined] {
  const idx = s.lastIndexOf(sep);
  return idx === -1 ? [s, undefined] : [s.slice(0, idx), s.slice(idx + 1)];
}

/**
 * Pull a Helm OCI chart. Returns the raw tar+gzip bytes of the layer.
 * Consume via `unpackTgz` (from `./tar`) or the high-level `inspectChart`.
 */
export async function pullChart(ref: string, opts: PullChartOptions = {}): Promise<Uint8Array> {
  const { host, repository, tag } = parseOciRef(ref);
  const maxBytes = opts.maxBytes ?? DEFAULT_MAX_BYTES;
  const creds = opts.auth?.[host];
  const authHeader = await resolveAuthHeader(host, repository, creds, opts.signal);

  const manifestUrl = `https://${host}/v2/${repository}/manifests/${tag}`;
  const manifestResp = await fetch(manifestUrl, {
    method: 'GET',
    headers: {
      Accept: MANIFEST_ACCEPT,
      ...(authHeader ? { Authorization: authHeader } : {}),
    },
    signal: opts.signal,
  });
  if (!manifestResp.ok) {
    throw new OciPullError(
      `manifest ${manifestResp.status} ${manifestResp.statusText} for ${ref}`,
    );
  }
  const manifest = (await manifestResp.json()) as OciManifest;
  const layer = selectChartLayer(manifest);
  if (layer.size > maxBytes) {
    throw new OciPullError(
      `advertised layer size ${layer.size} > limit ${maxBytes} — override with maxBytes`,
    );
  }

  const blobUrl = `https://${host}/v2/${repository}/blobs/${layer.digest}`;
  const blobResp = await fetch(blobUrl, {
    headers: authHeader ? { Authorization: authHeader } : {},
    signal: opts.signal,
  });
  if (!blobResp.ok) {
    throw new OciPullError(
      `layer ${blobResp.status} ${blobResp.statusText} for ${layer.digest}`,
    );
  }
  const buf = await blobResp.arrayBuffer();
  if (buf.byteLength > maxBytes) {
    throw new OciPullError(`received ${buf.byteLength} bytes, over limit ${maxBytes}`);
  }
  return new Uint8Array(buf);
}

interface OciManifest {
  layers: { mediaType: string; digest: string; size: number }[];
}

function selectChartLayer(manifest: OciManifest): {
  mediaType: string;
  digest: string;
  size: number;
} {
  if (!manifest.layers || manifest.layers.length === 0) {
    throw new OciPullError('manifest has no layers');
  }
  // Prefer the explicit Helm chart layer media type; fall back to the
  // first layer when the registry doesn't tag it (some do not).
  const tagged = manifest.layers.find((l) => l.mediaType === HELM_LAYER_MEDIA_TYPE);
  return tagged ?? manifest.layers[0]!;
}

/**
 * Compute the `Authorization` header for a given host. Four paths:
 *
 * 1. Caller supplied a bearer token → use it as-is.
 * 2. Caller supplied basic-auth creds → base64-encode and return.
 * 3. No creds → probe the manifest URL anonymously; on 401 with
 *    `WWW-Authenticate: Bearer`, follow the spec's token-exchange to
 *    get an anonymous read-only token.
 * 4. No creds, no challenge → no header, caller tries anonymous.
 */
async function resolveAuthHeader(
  host: string,
  repository: string,
  creds: OciCredentials | undefined,
  signal: AbortSignal | undefined,
): Promise<string | null> {
  if (creds) {
    if ('token' in creds) return `Bearer ${creds.token}`;
    const basic = base64Encode(`${creds.username}:${creds.password}`);
    return `Basic ${basic}`;
  }
  // Anonymous probe — mirrors fetch.rs::anonymous_bearer_for_public_pull.
  const probeUrl = `https://${host}/v2/${repository}/manifests/latest`;
  let probe: Response;
  try {
    probe = await fetch(probeUrl, { method: 'HEAD', signal });
  } catch {
    return null;
  }
  if (probe.status !== 401) return null;
  const challenge = probe.headers.get('WWW-Authenticate');
  if (!challenge) return null;
  const parsed = parseBearerChallenge(challenge);
  if (!parsed) return null;

  const tokenUrl = new URL(parsed.realm);
  if (parsed.service) tokenUrl.searchParams.set('service', parsed.service);
  tokenUrl.searchParams.set('scope', parsed.scope ?? `repository:${repository}:pull`);
  let tokenResp: Response;
  try {
    tokenResp = await fetch(tokenUrl, { signal });
  } catch {
    return null;
  }
  if (!tokenResp.ok) return null;
  const tokenBody = (await tokenResp.json()) as { token?: string; access_token?: string };
  const token = tokenBody.token ?? tokenBody.access_token;
  return token ? `Bearer ${token}` : null;
}

interface BearerChallenge {
  realm: string;
  service?: string;
  scope?: string;
}

/**
 * Parse `WWW-Authenticate: Bearer realm="…",service="…",scope="…"` per
 * RFC 6750 §3. Values may be quoted; keys are case-insensitive; unknown
 * keys are ignored.
 */
export function parseBearerChallenge(header: string): BearerChallenge | null {
  const match = header.match(/^Bearer\s+(.*)$/i);
  if (!match) return null;
  const body = match[1]!;
  const params: Record<string, string> = {};
  for (const part of body.split(',')) {
    const eq = part.indexOf('=');
    if (eq === -1) continue;
    const key = part.slice(0, eq).trim().toLowerCase();
    const raw = part.slice(eq + 1).trim();
    const value = raw.startsWith('"') && raw.endsWith('"') ? raw.slice(1, -1) : raw;
    params[key] = value;
  }
  if (!params.realm) return null;
  return { realm: params.realm, service: params.service, scope: params.scope };
}

function base64Encode(s: string): string {
  // Works in Node (Buffer exists) and browsers (btoa exists).
  if (typeof Buffer !== 'undefined') {
    return Buffer.from(s, 'utf8').toString('base64');
  }
  return btoa(unescape(encodeURIComponent(s)));
}
