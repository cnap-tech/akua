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
): AsyncGenerator<{ path: string; bytes: Uint8Array }, void, undefined> {
  const source = toReadableStream(tgz).pipeThrough(new DecompressionStream('gzip'));
  const reader = source.getReader();
  let buf = new Uint8Array(0);

  /** Pull until `buf` has at least `needed` bytes, or the stream ends. */
  const ensure = async (needed: number): Promise<boolean> => {
    while (buf.length < needed) {
      const { value, done } = await reader.read();
      if (done) return buf.length >= needed;
      buf = concatPair(buf, value);
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
export async function unpackTgz(tgz: TgzInput): Promise<Map<string, Uint8Array>> {
  const out = new Map<string, Uint8Array>();
  for await (const { path, bytes } of streamTgzEntries(tgz)) {
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
  return raw.pipeThrough(new CompressionStream('gzip'));
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

/**
 * Minimal YAML-subset parser for the exact shape of Helm `Chart.yaml`
 * and `.akua/metadata.yaml` — scalar keys, nested mappings, string /
 * number / bool values, block-style lists of mappings. Not a full YAML
 * parser; using one (e.g. `yaml` npm) would balloon the SDK size and
 * pull in a regex-heavy dep.
 *
 * For anything more complex than those two files, consumers should
 * bring their own parser.
 */
function parseYaml(text: string): unknown {
  // Quick JSON escape hatch — some of our Chart.yaml files are
  // JSON-compatible and that's fast + safe. Try it first.
  const trimmed = text.trim();
  if (trimmed.startsWith('{') || trimmed.startsWith('[')) {
    try {
      return JSON.parse(trimmed);
    } catch {
      // fall through to YAML parser
    }
  }
  return parseYamlMapping(text.split(/\r?\n/));
}

function parseYamlMapping(lines: string[]): Record<string, unknown> {
  const root: Record<string, unknown> = {};
  parseYamlBlock(lines, 0, 0, root);
  return root;
}

function parseYamlBlock(
  lines: string[],
  startLine: number,
  indent: number,
  out: Record<string, unknown> | unknown[],
): number {
  let i = startLine;
  while (i < lines.length) {
    const raw = lines[i]!;
    if (raw.trim() === '' || raw.trim().startsWith('#')) {
      i++;
      continue;
    }
    const leading = raw.length - raw.trimStart().length;
    if (leading < indent) return i;
    const line = raw.slice(leading);
    if (Array.isArray(out)) {
      if (line.startsWith('- ')) {
        const rest = line.slice(2);
        if (rest.includes(':')) {
          const item: Record<string, unknown> = {};
          const [k, v] = splitKV(rest);
          if (v !== undefined && v !== '') {
            item[k] = coerceScalar(v);
          } else {
            item[k] = {};
          }
          i = parseYamlBlock(lines, i + 1, leading + 2, item);
          // second-level mapping inside list — item may still be empty
          out.push(item);
        } else {
          out.push(coerceScalar(rest));
          i++;
        }
      } else {
        return i;
      }
    } else {
      const [k, v] = splitKV(line);
      if (v === undefined) {
        i++;
        continue;
      }
      if (v === '') {
        // Look ahead: `-` starts a list, otherwise a nested mapping.
        const next = findNextContent(lines, i + 1);
        if (next !== -1 && lines[next]!.trimStart().startsWith('- ')) {
          const list: unknown[] = [];
          (out as Record<string, unknown>)[k] = list;
          i = parseYamlBlock(lines, i + 1, leading + 2, list);
        } else {
          const child: Record<string, unknown> = {};
          (out as Record<string, unknown>)[k] = child;
          i = parseYamlBlock(lines, i + 1, leading + 2, child);
        }
      } else {
        (out as Record<string, unknown>)[k] = coerceScalar(v);
        i++;
      }
    }
  }
  return i;
}

function splitKV(line: string): [string, string | undefined] {
  const idx = line.indexOf(':');
  if (idx === -1) return [line, undefined];
  const key = line.slice(0, idx).trim();
  const val = line.slice(idx + 1).trim();
  return [key, val];
}

function coerceScalar(value: string): unknown {
  // Strip a line-ending comment — safe because our YAML inputs are
  // machine-generated and never contain `#` inside scalars.
  const hashIdx = value.indexOf(' #');
  const raw = (hashIdx >= 0 ? value.slice(0, hashIdx) : value).trim();
  if (raw === '') return '';
  if (raw === 'true') return true;
  if (raw === 'false') return false;
  if (raw === 'null' || raw === '~') return null;
  if (/^-?\d+$/.test(raw)) return parseInt(raw, 10);
  if (/^-?\d+\.\d+$/.test(raw)) return parseFloat(raw);
  if (raw.startsWith('"') && raw.endsWith('"')) return raw.slice(1, -1);
  if (raw.startsWith("'") && raw.endsWith("'")) return raw.slice(1, -1);
  return raw;
}

function findNextContent(lines: string[], from: number): number {
  for (let i = from; i < lines.length; i++) {
    const t = lines[i]!.trim();
    if (t !== '' && !t.startsWith('#')) return i;
  }
  return -1;
}
