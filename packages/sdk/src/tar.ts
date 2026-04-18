/**
 * Pure-TypeScript tar.gz reader/writer. Works in Node 22+ and modern
 * browsers via `DecompressionStream('gzip')` / `CompressionStream('gzip')`
 * — no npm dependency on `tar-stream`, `pako`, `js-untar`, etc.
 *
 * Scope: ustar-format regular files + directories. That's what every
 * `helm package` tarball uses. GNU-extended long names aren't handled
 * because no real-world Helm chart produces them (paths stay inside a
 * single top-level `<chart-name>/` directory + standard subdirs).
 *
 * Safety: `unpackTgz` rejects absolute paths and `..` components the
 * same way `akua-core::fetch::validate_tar_entry_path` does, so a
 * malicious chart can't write outside the caller's logical scope.
 */

import { parse as parseYaml } from 'yaml';

import { AkuaError } from './errors.js';

export class TarError extends AkuaError {
  constructor(message: string) {
    super(message);
    this.name = 'TarError';
  }
}

/** Block size used by the tar format — every header + every data segment is padded to this. */
const BLOCK = 512;

/** Accepted shapes for the tgz input. Pick what's cheapest at the call-site. */
export type TgzInput = Uint8Array | ArrayBuffer | Blob | ReadableStream<Uint8Array>;

export interface StreamTgzOptions {
  /**
   * Maximum number of entries. Prevents an attacker-crafted archive
   * with millions of empty entries from exhausting CPU. Default 20 000
   * (matches Rust `AKUA_MAX_TAR_ENTRIES`).
   */
  maxEntries?: number;
  /**
   * Maximum total decompressed bytes yielded. Protects against gzip
   * bombs. Default 500 MB (matches Rust `AKUA_MAX_EXTRACTED_BYTES`).
   */
  maxTotalBytes?: number;
  /**
   * Maximum bytes for any single entry. Default 100 MB. Stops an entry
   * bigger than a reasonable chart file from landing in memory.
   */
  maxEntryBytes?: number;
}

const DEFAULT_MAX_ENTRIES = 20_000;
const DEFAULT_MAX_TOTAL_BYTES = 500 * 1024 * 1024;
const DEFAULT_MAX_ENTRY_BYTES = 100 * 1024 * 1024;

/**
 * Stream tar+gzip entries as they arrive. Yields `{ path, bytes }` one
 * entry at a time; peak memory is one entry's decompressed size + a
 * sub-block read buffer (~tens of KB), independent of archive size.
 *
 * Prefer this over [`unpackTgz`] when the archive may be large or when
 * the consumer only cares about specific entries (can break early and
 * the rest of the gzip stream is discarded).
 */
export async function* streamTgzEntries(
  tgz: TgzInput,
  options: StreamTgzOptions = {},
): AsyncGenerator<{ path: string; bytes: Uint8Array }, void, undefined> {
  const maxEntries = options.maxEntries ?? DEFAULT_MAX_ENTRIES;
  const maxTotalBytes = options.maxTotalBytes ?? DEFAULT_MAX_TOTAL_BYTES;
  const maxEntryBytes = options.maxEntryBytes ?? DEFAULT_MAX_ENTRY_BYTES;
  // DecompressionStream's type surface changed between TS lib versions:
  // it expects `Uint8Array<ArrayBuffer>` in some and `BufferSource` in
  // others. The runtime is compatible across the board — cast to silence
  // the variance without losing type safety for callers.
  const source = toReadableStream(tgz).pipeThrough(
    new DecompressionStream('gzip') as unknown as ReadableWritablePair<Uint8Array, Uint8Array>,
  );
  const reader = source.getReader();
  let buf: Uint8Array = new Uint8Array(0);
  let entryCount = 0;
  let totalBytes = 0;

  /** Pull until `buf` has at least `needed` bytes, or the stream ends. */
  const ensure = async (needed: number): Promise<boolean> => {
    while (buf.length < needed) {
      const { value, done } = await reader.read();
      if (done) return buf.length >= needed;
      buf = concatPair(buf, value as Uint8Array);
    }
    return true;
  };

  try {
    while (true) {
      if (!(await ensure(BLOCK))) return; // clean EOF before a header
      const header = buf.subarray(0, BLOCK);
      if (isZeroBlock(header)) return;
      const name = readString(header, 0, 100);
      const size = readOctalSize(header, 124, 12);
      const typeflag = String.fromCharCode(header[156] ?? 0);
      const prefix = readString(header, 345, 155);
      const fullPath = prefix ? `${prefix}/${name}` : name;

      // Advance past the header before any awaits so a thrown
      // validation error doesn't leave us resyncing from the middle
      // of a data segment on retry (consumers should just stop, but
      // the invariant is cleaner this way).
      buf = buf.subarray(BLOCK);

      if (!name) continue;
      validateEntryPath(fullPath);

      if (++entryCount > maxEntries) {
        throw new TarError(`tar entry count exceeded limit (${maxEntries})`);
      }
      if (size > maxEntryBytes) {
        throw new TarError(
          `tar entry ${fullPath} size ${size} exceeds per-entry limit ${maxEntryBytes}`,
        );
      }
      totalBytes += size;
      if (totalBytes > maxTotalBytes) {
        throw new TarError(
          `tar total decompressed bytes ${totalBytes} exceeds limit ${maxTotalBytes}`,
        );
      }

      const padded = roundUp(size, BLOCK);
      if (!(await ensure(padded))) {
        throw new TarError(`truncated tar: entry ${fullPath} expected ${size} bytes`);
      }
      const data = buf.subarray(0, size);
      buf = buf.subarray(padded);

      // Regular files only: 0 / '0' / '' (old tar). Links, longname
      // extensions, global/file headers are skipped — real helm charts
      // never contain them.
      if (typeflag === '0' || typeflag === '' || typeflag === '\0') {
        // Copy so the consumer can hold onto it across our next read.
        yield { path: fullPath, bytes: new Uint8Array(data) };
      }
    }
  } finally {
    await reader.cancel().catch(() => {});
  }
}

/**
 * Unpack a tar+gzip archive into a map of `path -> bytes`. Convenience
 * over [`streamTgzEntries`] — materialises every entry in memory, so
 * peak usage is the full unpacked archive. Prefer the streaming API
 * for anything bigger than a typical Helm chart.
 */
export async function unpackTgz(
  tgz: TgzInput,
  options: StreamTgzOptions = {},
): Promise<Map<string, Uint8Array>> {
  const out = new Map<string, Uint8Array>();
  for await (const { path, bytes } of streamTgzEntries(tgz, options)) {
    out.set(path, bytes);
  }
  return out;
}

/**
 * High-level: read a chart's metadata out of a tgz. Looks up the single
 * top-level directory (helm convention), then grabs `Chart.yaml`,
 * `values.schema.json` (optional), and `.akua/metadata.yaml` (optional).
 */
export async function inspectChartBytes(tgz: TgzInput): Promise<{
  chartYaml: Record<string, unknown>;
  valuesSchema: Record<string, unknown> | null;
  akuaMetadata: Record<string, unknown> | null;
}> {
  // Stream entries and keep only the three files we care about. Peak
  // memory is Chart.yaml + maybe schema + maybe metadata — bounded by
  // those three even for a 100 MB chart with huge templates.
  let topDir: string | null = null;
  let chartYamlBytes: Uint8Array | null = null;
  let schemaBytes: Uint8Array | null = null;
  let metaBytes: Uint8Array | null = null;

  for await (const { path, bytes } of streamTgzEntries(tgz)) {
    const slash = path.indexOf('/');
    if (slash === -1) continue;
    const dir = path.slice(0, slash);
    const rest = path.slice(slash + 1);
    if (topDir === null) topDir = dir;
    else if (topDir !== dir) continue; // ignore strays outside the single top-level dir
    if (rest === 'Chart.yaml') chartYamlBytes = bytes;
    else if (rest === 'values.schema.json') schemaBytes = bytes;
    else if (rest === '.akua/metadata.yaml') metaBytes = bytes;

    // Early exit — once we have all three interesting files we can
    // stop pulling the gzip stream. `streamTgzEntries` cancels the
    // underlying reader in its `finally`.
    if (chartYamlBytes && schemaBytes && metaBytes) break;
  }

  if (!topDir) throw new TarError('chart tarball has no top-level directory');
  if (!chartYamlBytes) throw new TarError(`chart tarball missing ${topDir}/Chart.yaml`);

  const chartYaml = parseYaml(decode(chartYamlBytes)) as Record<string, unknown>;
  const valuesSchema = schemaBytes
    ? (JSON.parse(decode(schemaBytes)) as Record<string, unknown>)
    : null;
  const akuaMetadata = metaBytes
    ? (parseYaml(decode(metaBytes)) as Record<string, unknown>)
    : null;
  return { chartYaml, valuesSchema, akuaMetadata };
}

/**
 * Pack a map of path→bytes back into a tar+gzip stream. Paths are
 * written under `rootDir/` (helm chart convention). Useful for the
 * inverse of `inspectChartBytes` — assemble a chart from in-memory
 * pieces and hand the buffer straight to `helm install` or OCI push.
 *
 * Limits: regular files only; no symlinks, no long names, no dirs
 * (receivers create dirs implicitly). Modes default to 0644.
 */
/** Iterable shape accepted by [`packTgzStream`]. */
export type PackEntries =
  | Iterable<readonly [string, Uint8Array]>
  | AsyncIterable<readonly [string, Uint8Array]>;

/**
 * Streaming pack: emit the tar+gzip archive as a `ReadableStream` that
 * produces chunks on demand. Consumers can pipe straight to disk, to
 * an HTTP upload, or to an OCI layer push without ever materialising
 * the full `.tgz` in memory. Peak memory is one entry's data + the
 * compressor's internal window — chart size doesn't scale it.
 *
 * Entries may be provided as a sync or async iterable of
 * `[path, bytes]` tuples. Async iterables let the caller stream
 * subchart bytes from network pulls directly through the packer
 * without a `Map<>` intermediate.
 */
export function packTgzStream(rootDir: string, entries: PackEntries): ReadableStream<Uint8Array> {
  const root = rootDir.replace(/\/+$/, '');
  const raw = new ReadableStream<Uint8Array>({
    async start(controller) {
      try {
        const iter = isAsyncIterable(entries)
          ? (entries as AsyncIterable<readonly [string, Uint8Array]>)
          : (toAsyncIterable(entries as Iterable<readonly [string, Uint8Array]>));
        for await (const [path, data] of iter) {
          validateEntryPath(path);
          const fullPath = `${root}/${path.replace(/^\/+/, '')}`;
          controller.enqueue(buildHeader(fullPath, data.length));
          controller.enqueue(data);
          const rem = data.length % BLOCK;
          if (rem !== 0) controller.enqueue(new Uint8Array(BLOCK - rem));
        }
        // Two zero blocks mark end-of-archive.
        controller.enqueue(new Uint8Array(BLOCK));
        controller.enqueue(new Uint8Array(BLOCK));
        controller.close();
      } catch (err) {
        controller.error(err);
      }
    },
  });
  return raw.pipeThrough(
    new CompressionStream('gzip') as unknown as ReadableWritablePair<Uint8Array, Uint8Array>,
  );
}

/**
 * Convenience: collect [`packTgzStream`] into a single `Uint8Array`.
 * Prefer the stream API for large charts — this version buffers the
 * whole archive before returning.
 */
export async function packTgz(rootDir: string, entries: PackEntries): Promise<Uint8Array> {
  const stream = packTgzStream(rootDir, entries);
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

// ---------------------------------------------------------------------------
// Header parsing / writing
// ---------------------------------------------------------------------------

function readString(buf: Uint8Array, offset: number, len: number): string {
  let end = offset;
  const stop = offset + len;
  while (end < stop && buf[end] !== 0) end++;
  return decode(buf.subarray(offset, end));
}

function readOctalSize(buf: Uint8Array, offset: number, len: number): number {
  const raw = readString(buf, offset, len).trim();
  if (raw === '') return 0;
  const n = parseInt(raw, 8);
  if (!Number.isFinite(n) || n < 0) {
    throw new TarError(`invalid octal size field: "${raw}"`);
  }
  return n;
}

function buildHeader(path: string, size: number): Uint8Array {
  if (path.length > 100) {
    // Tar format reserves 100 bytes for the name + 155 for an optional
    // path prefix (ustar). For simplicity we reject long paths — real
    // helm charts stay well under the limit.
    throw new TarError(`path too long for ustar header: ${path}`);
  }
  const header = new Uint8Array(BLOCK);
  writeString(header, path, 0, 100);
  writeOctal(header, 0o644, 100, 8); // mode
  writeOctal(header, 0, 108, 8); // uid
  writeOctal(header, 0, 116, 8); // gid
  writeOctal(header, size, 124, 12);
  writeOctal(header, 0, 136, 12); // mtime — zero for determinism
  // checksum: start as all-spaces, fill at the end
  for (let i = 148; i < 156; i++) header[i] = 0x20;
  header[156] = 0x30; // typeflag '0' — regular file
  writeString(header, 'ustar', 257, 6);
  writeString(header, '00', 263, 2);
  // compute + write checksum
  let sum = 0;
  for (const b of header) sum += b;
  writeOctal(header, sum, 148, 7);
  header[155] = 0;
  return header;
}

function writeString(buf: Uint8Array, value: string, offset: number, len: number) {
  const bytes = encode(value);
  for (let i = 0; i < Math.min(bytes.length, len); i++) buf[offset + i] = bytes[i]!;
}

function writeOctal(buf: Uint8Array, value: number, offset: number, len: number) {
  // Octal with trailing NUL, except the checksum field uses space.
  const octal = value.toString(8).padStart(len - 1, '0');
  writeString(buf, octal, offset, len - 1);
  buf[offset + len - 1] = 0;
}

// ---------------------------------------------------------------------------
// Validation (mirrors akua-core::fetch::validate_tar_entry_path)
// ---------------------------------------------------------------------------

function validateEntryPath(path: string): void {
  if (path.startsWith('/') || path.startsWith('\\')) {
    throw new TarError(`path traversal: absolute entry ${path}`);
  }
  if (/^[A-Za-z]:[\\/]/.test(path)) {
    throw new TarError(`path traversal: drive-prefixed entry ${path}`);
  }
  for (const seg of path.split(/[\\/]/)) {
    if (seg === '..') {
      throw new TarError(`path traversal: .. component in ${path}`);
    }
  }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function detectTopLevelDir(paths: Iterable<string>): string | null {
  const roots = new Set<string>();
  for (const p of paths) {
    const slash = p.indexOf('/');
    if (slash === -1) continue;
    roots.add(p.slice(0, slash));
  }
  if (roots.size !== 1) return null;
  return roots.values().next().value!;
}

function isZeroBlock(block: Uint8Array): boolean {
  for (const b of block) if (b !== 0) return false;
  return true;
}

function roundUp(n: number, to: number): number {
  return Math.ceil(n / to) * to;
}

function concat(parts: Uint8Array[]): Uint8Array {
  const total = parts.reduce((n, p) => n + p.length, 0);
  const out = new Uint8Array(total);
  let off = 0;
  for (const p of parts) {
    out.set(p, off);
    off += p.length;
  }
  return out;
}

/** Cheap two-array concat — hot-path in the streaming parser's buffer refill. */
function concatPair(a: Uint8Array, b: Uint8Array): Uint8Array {
  if (a.length === 0) return b;
  if (b.length === 0) return a;
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}

function isAsyncIterable<T>(x: unknown): x is AsyncIterable<T> {
  return x != null && typeof (x as { [Symbol.asyncIterator]?: unknown })[Symbol.asyncIterator] === 'function';
}

async function* toAsyncIterable<T>(sync: Iterable<T>): AsyncGenerator<T> {
  for (const x of sync) yield x;
}

/** Normalise any accepted tgz input shape to a byte-producing ReadableStream. */
function toReadableStream(input: TgzInput): ReadableStream<Uint8Array> {
  if (input instanceof ReadableStream) return input;
  if (input instanceof Blob) return input.stream() as unknown as ReadableStream<Uint8Array>;
  if (input instanceof ArrayBuffer) {
    return new Blob([input]).stream() as unknown as ReadableStream<Uint8Array>;
  }
  return new Blob([input as unknown as BlobPart]).stream() as unknown as ReadableStream<Uint8Array>;
}

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder('utf-8');

function encode(s: string): Uint8Array {
  return textEncoder.encode(s);
}
function decode(b: Uint8Array): string {
  return textDecoder.decode(b);
}

async function gunzip(bytes: Uint8Array): Promise<Uint8Array> {
  // Cast via unknown — newer TS narrows Blob ctor to ArrayBuffer-backed BlobPart,
  // but Uint8Array<ArrayBufferLike> is the practical input shape everywhere.
  const stream = new Blob([bytes as unknown as BlobPart]).stream().pipeThrough(
    new DecompressionStream('gzip'),
  );
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

async function gzip(bytes: Uint8Array): Promise<Uint8Array> {
  const stream = new Blob([bytes as unknown as BlobPart]).stream().pipeThrough(
    new CompressionStream('gzip'),
  );
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

