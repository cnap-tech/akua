import { describe, it, expect } from 'vitest';

import {
  packTgz,
  packTgzStream,
  unpackTgz,
  streamTgzEntries,
  inspectChartBytes,
  TarError,
} from '../src/tar.ts';

describe('packTgz + unpackTgz round-trip', () => {
  it('round-trips a minimal chart layout', async () => {
    const entries = new Map<string, Uint8Array>([
      ['Chart.yaml', bytes('apiVersion: v2\nname: demo\nversion: 0.1.0\n')],
      ['values.yaml', bytes('replicaCount: 1\n')],
      ['templates/deployment.yaml', bytes('kind: Deployment\n')],
    ]);
    const tgz = await packTgz('demo', entries);
    const unpacked = await unpackTgz(tgz);
    expect(unpacked.get('demo/Chart.yaml')).toEqual(bytes('apiVersion: v2\nname: demo\nversion: 0.1.0\n'));
    expect(unpacked.get('demo/values.yaml')).toEqual(bytes('replicaCount: 1\n'));
    expect(unpacked.get('demo/templates/deployment.yaml')).toEqual(bytes('kind: Deployment\n'));
  });

  it('preserves binary content through the gzip roundtrip', async () => {
    const payload = new Uint8Array(1024);
    for (let i = 0; i < payload.length; i++) payload[i] = (i * 17) & 0xff;
    const tgz = await packTgz('bin', new Map([['blob.dat', payload]]));
    const unpacked = await unpackTgz(tgz);
    expect(unpacked.get('bin/blob.dat')).toEqual(payload);
  });
});

describe('unpackTgz safety', () => {
  it('rejects absolute paths', async () => {
    // Manually craft a tar with an absolute entry name, gzip, then feed to unpackTgz.
    const malicious = makeTar([['/etc/passwd', bytes('pwned')]]);
    const tgz = await gzipIt(malicious);
    await expect(unpackTgz(tgz)).rejects.toThrow(TarError);
  });

  it('rejects .. components', async () => {
    const malicious = makeTar([['chart/../evil.yaml', bytes('pwned')]]);
    const tgz = await gzipIt(malicious);
    await expect(unpackTgz(tgz)).rejects.toThrow(TarError);
  });
});

describe('inspectChartBytes', () => {
  it('returns chartYaml/valuesSchema/akuaMetadata from a packed chart', async () => {
    const schema = { type: 'object', properties: { foo: { type: 'string' } } };
    const meta = {
      akua: { version: '0.0.0', buildTime: '2026-04-18T00:00:00Z' },
      sources: [],
      transforms: [],
    };
    const tgz = await packTgz(
      'demo',
      new Map([
        ['Chart.yaml', bytes('apiVersion: v2\nname: demo\nversion: 0.1.0\n')],
        ['values.schema.json', bytes(JSON.stringify(schema))],
        ['.akua/metadata.yaml', bytes(JSON.stringify(meta))], // JSON is valid YAML
      ]),
    );
    const out = await inspectChartBytes(tgz);
    expect(out.chartYaml.name).toBe('demo');
    expect(out.valuesSchema).toEqual(schema);
    expect(out.akuaMetadata).toEqual(meta);
  });

  it('returns nulls for chartYaml-only vanilla Helm charts', async () => {
    const tgz = await packTgz(
      'vanilla',
      new Map([['Chart.yaml', bytes('apiVersion: v2\nname: vanilla\nversion: 0.1.0\n')]]),
    );
    const out = await inspectChartBytes(tgz);
    expect(out.chartYaml.name).toBe('vanilla');
    expect(out.valuesSchema).toBeNull();
    expect(out.akuaMetadata).toBeNull();
  });
});

describe('streamTgzEntries', () => {
  it('yields entries one at a time and accepts a ReadableStream input', async () => {
    const tgz = await packTgz(
      'demo',
      new Map([
        ['a.txt', bytes('a')],
        ['b.txt', bytes('bb')],
        ['c.txt', bytes('ccc')],
      ]),
    );
    // Feed the packed tgz back in as a stream, not a Uint8Array — confirms
    // the generator handles both input shapes.
    const stream = new Blob([tgz]).stream() as unknown as ReadableStream<Uint8Array>;
    const seen: Array<[string, number]> = [];
    for await (const entry of streamTgzEntries(stream)) {
      seen.push([entry.path, entry.bytes.length]);
    }
    expect(seen).toEqual([
      ['demo/a.txt', 1],
      ['demo/b.txt', 2],
      ['demo/c.txt', 3],
    ]);
  });

  it('supports early break — consumer takes only what it needs', async () => {
    const tgz = await packTgz(
      'demo',
      new Map([
        ['Chart.yaml', bytes('name: demo\n')],
        ['templates/a.yaml', bytes('kind: A\n')],
        ['templates/b.yaml', bytes('kind: B\n')],
      ]),
    );
    let first: string | null = null;
    for await (const entry of streamTgzEntries(tgz)) {
      first = entry.path;
      break; // should cancel the gzip stream cleanly
    }
    expect(first).toBe('demo/Chart.yaml');
  });
});

describe('packTgzStream', () => {
  it('accepts async iterables and round-trips via unpackTgz', async () => {
    async function* entries() {
      yield ['Chart.yaml', bytes('name: async\n')] as const;
      yield ['values.yaml', bytes('x: 1\n')] as const;
    }
    const stream = packTgzStream('async', entries());
    const tgz = new Uint8Array(await new Response(stream).arrayBuffer());
    const unpacked = await unpackTgz(tgz);
    expect(unpacked.get('async/Chart.yaml')).toEqual(bytes('name: async\n'));
    expect(unpacked.get('async/values.yaml')).toEqual(bytes('x: 1\n'));
  });

  it('bubbles errors from the entry iterable into the stream', async () => {
    async function* entries() {
      yield ['ok.yaml', bytes('a\n')] as const;
      throw new Error('upstream fetch failed');
    }
    const stream = packTgzStream('fail', entries());
    await expect(new Response(stream).arrayBuffer()).rejects.toThrow('upstream fetch failed');
  });
});

// --- helpers -------------------------------------------------------

function bytes(s: string): Uint8Array {
  return new TextEncoder().encode(s);
}

async function gzipIt(data: Uint8Array): Promise<Uint8Array> {
  const stream = new Blob([data]).stream().pipeThrough(new CompressionStream('gzip'));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

/**
 * Hand-craft a tar stream with arbitrary (possibly-malicious) paths.
 * Bypasses `packTgz`'s validation so we can test `unpackTgz`'s guards.
 */
function makeTar(files: Array<[string, Uint8Array]>): Uint8Array {
  const parts: Uint8Array[] = [];
  for (const [name, body] of files) {
    const header = new Uint8Array(512);
    // name at offset 0..100
    const nameBytes = new TextEncoder().encode(name);
    for (let i = 0; i < Math.min(nameBytes.length, 100); i++) header[i] = nameBytes[i]!;
    // size at 124..136 — 12-byte octal
    const size = body.length.toString(8).padStart(11, '0');
    for (let i = 0; i < 11; i++) header[124 + i] = size.charCodeAt(i);
    // typeflag '0' at 156
    header[156] = 0x30;
    // checksum placeholder (spaces)
    for (let i = 148; i < 156; i++) header[i] = 0x20;
    let sum = 0;
    for (const b of header) sum += b;
    const cksum = sum.toString(8).padStart(6, '0');
    for (let i = 0; i < 6; i++) header[148 + i] = cksum.charCodeAt(i);
    header[154] = 0;
    header[155] = 0x20;
    parts.push(header, body);
    const rem = body.length % 512;
    if (rem !== 0) parts.push(new Uint8Array(512 - rem));
  }
  parts.push(new Uint8Array(512), new Uint8Array(512));
  let total = 0;
  for (const p of parts) total += p.length;
  const out = new Uint8Array(total);
  let off = 0;
  for (const p of parts) {
    out.set(p, off);
    off += p.length;
  }
  return out;
}
