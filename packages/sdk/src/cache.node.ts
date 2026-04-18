/**
 * Node-only on-disk cache for `pullChart`. Shares the layout with the
 * CLI's `$XDG_CACHE_HOME/akua/v1/` so a worker that already ran `akua`
 * warms the SDK's cache (and vice versa).
 *
 * Layout:
 * ```
 *   refs/<sha256(key)>        → hex sha256 of the cached blob
 *   blobs/<sha256(bytes)>.tgz → the chart tarball
 * ```
 * `key` is `${repo}|${name}|${version}` — matches Rust
 * `akua-core::fetch::cache::key_for_dep`. Integrity-checked on read;
 * corrupt entries fall through to a live pull.
 *
 * Disabled by setting `AKUA_NO_CACHE=1` in the environment.
 */

import { createHash } from 'node:crypto';
import { createWriteStream, existsSync } from 'node:fs';
import { mkdir, readFile, rename, stat } from 'node:fs/promises';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';

import { pullChart, type PullChartOptions } from './oci.js';
import { parseOciRef } from './oci.js';
import { parseHelmHttpRef } from './helm-http.js';

/**
 * Pull a chart, consulting the on-disk cache first. On hit, returns
 * cached bytes without any network round-trip. On miss, calls
 * [`pullChart`] and writes the result back to the cache atomically.
 *
 * Bypass by setting `AKUA_NO_CACHE=1`.
 */
export async function pullChartCached(
  ref: string,
  opts: PullChartOptions = {},
): Promise<Uint8Array> {
  if (process.env.AKUA_NO_CACHE) {
    return pullChart(ref, opts);
  }
  const key = cacheKeyFor(ref);
  const hit = await readCached(key);
  if (hit) return hit;

  const bytes = await pullChart(ref, opts);
  await writeCached(key, bytes).catch(() => {
    // Cache writes are best-effort. A full disk shouldn't fail the pull.
  });
  return bytes;
}

/** Compute the cache key for a chart ref. Matches Rust `key_for_dep`. */
export function cacheKeyFor(ref: string): string {
  if (ref.startsWith('oci://')) {
    const { host, repository, tag } = parseOciRef(ref);
    const segments = repository.split('/').filter(Boolean);
    const name = segments[segments.length - 1]!;
    const parent = segments.slice(0, -1).join('/');
    const repo = parent ? `oci://${host}/${parent}` : `oci://${host}`;
    return `${repo}|${name}|${tag}`;
  }
  if (ref.startsWith('https://') || ref.startsWith('http://')) {
    const { repo, chart, version } = parseHelmHttpRef(ref);
    return `${repo}|${chart}|${version}`;
  }
  throw new Error(`unsupported ref scheme: ${ref}`);
}

/** Resolve the cache root. Matches Rust's resolution order. */
export function cacheRoot(): string | null {
  const override = process.env.AKUA_CACHE_DIR;
  if (override) return join(override, 'v1');
  const xdg = process.env.XDG_CACHE_HOME;
  if (xdg) return join(xdg, 'akua', 'v1');
  const home = process.env.HOME ?? homedir();
  if (!home) return null;
  return join(home, '.cache', 'akua', 'v1');
}

async function readCached(key: string): Promise<Uint8Array | null> {
  const root = cacheRoot();
  if (!root) return null;
  try {
    const keyHash = sha256Hex(Buffer.from(key, 'utf8'));
    const refPath = join(root, 'refs', keyHash);
    const blobDigest = (await readFile(refPath, 'utf8')).trim();
    if (!isHex64(blobDigest)) return null;
    const blobPath = join(root, 'blobs', `${blobDigest}.tgz`);
    const bytes = await readFile(blobPath);
    // Integrity check — corrupt cache entry is worse than a miss.
    if (sha256Hex(bytes) !== blobDigest) return null;
    return new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  } catch {
    return null;
  }
}

async function writeCached(key: string, bytes: Uint8Array): Promise<void> {
  const root = cacheRoot();
  if (!root) return;
  const blobsDir = join(root, 'blobs');
  const refsDir = join(root, 'refs');
  await mkdir(blobsDir, { recursive: true });
  await mkdir(refsDir, { recursive: true });

  const blobDigest = sha256Hex(Buffer.from(bytes));
  const blobPath = join(blobsDir, `${blobDigest}.tgz`);
  if (!existsSync(blobPath)) {
    await atomicWrite(blobsDir, blobPath, bytes);
  }
  const keyHash = sha256Hex(Buffer.from(key, 'utf8'));
  const refPath = join(refsDir, keyHash);
  await atomicWrite(refsDir, refPath, Buffer.from(blobDigest, 'utf8'));
}

/** Write `bytes` to `finalPath` via a scratch tempfile rename. */
async function atomicWrite(scratchDir: string, finalPath: string, bytes: Uint8Array): Promise<void> {
  const tmpPath = join(
    scratchDir,
    `.tmp-${process.pid}-${Date.now()}-${Math.random().toString(36).slice(2)}`,
  );
  // Wrap in an array so `Readable.from` treats the whole Uint8Array as
  // a single chunk; iterating a Uint8Array yields numbers, which
  // `createWriteStream.write` rejects.
  await pipeline(Readable.from([Buffer.from(bytes)]), createWriteStream(tmpPath));
  await rename(tmpPath, finalPath);
}

function sha256Hex(bytes: Buffer | Uint8Array): string {
  return createHash('sha256').update(bytes).digest('hex');
}

function isHex64(s: string): boolean {
  return s.length === 64 && /^[0-9a-f]+$/.test(s);
}

// Keep these imports alive for TS — `tmpdir` / `stat` could be useful
// for future eviction logic; satisfy no-unused-imports until then.
void tmpdir;
void stat;
