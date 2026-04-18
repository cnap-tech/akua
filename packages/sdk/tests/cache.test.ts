import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { cacheKeyFor, pullChartCached } from '../src/cache.node.ts';
import { clearIndexCache } from '../src/helm-http.ts';

let dir: string;
const realFetch = globalThis.fetch;

beforeEach(async () => {
  dir = await mkdtemp(join(tmpdir(), 'akua-cache-'));
  clearIndexCache();
});

afterEach(async () => {
  await rm(dir, { recursive: true, force: true });
  globalThis.fetch = realFetch;
  delete process.env.AKUA_CACHE_DIR;
  delete process.env.AKUA_NO_CACHE;
});

describe('cacheKeyFor', () => {
  it('matches Rust key_for_dep for OCI refs (parent|chart|version)', () => {
    expect(cacheKeyFor('oci://ghcr.io/stefanprodan/charts/podinfo:6.7.1')).toBe(
      'oci://ghcr.io/stefanprodan/charts|podinfo|6.7.1',
    );
  });

  it('shallow OCI path yields oci://host|chart|version', () => {
    expect(cacheKeyFor('oci://registry.example.com/chart:1.0.0')).toBe(
      'oci://registry.example.com|chart|1.0.0',
    );
  });

  it('HTTP helm ref yields repo|chart|version', () => {
    expect(cacheKeyFor('https://charts.example.com/path/chart:1.0.0')).toBe(
      'https://charts.example.com/path|chart|1.0.0',
    );
  });
});

describe('pullChartCached', () => {
  function sha256Hex(bytes: Uint8Array): string {
    return createHash('sha256').update(bytes).digest('hex');
  }

  it('writes the fetched tgz to $AKUA_CACHE_DIR on first call', async () => {
    const bytes = new Uint8Array([0x1f, 0x8b, 0x42, 0x01]);
    const fetchMock = vi.fn(async (url: string | URL) => {
      const u = url.toString();
      if (u.includes('/manifests/')) {
        return new Response(
          JSON.stringify({
            layers: [
              {
                mediaType: 'application/vnd.cncf.helm.chart.content.v1.tar+gzip',
                digest: 'sha256:x',
                size: bytes.byteLength,
              },
            ],
          }),
          { status: 200 },
        );
      }
      return new Response(bytes, { status: 200 });
    });
    globalThis.fetch = fetchMock as unknown as typeof fetch;
    process.env.AKUA_CACHE_DIR = dir;

    const got = await pullChartCached('oci://reg.example.com/x:1.0.0');
    expect(got).toEqual(bytes);

    const digest = sha256Hex(bytes);
    expect(existsSync(join(dir, 'v1', 'blobs', `${digest}.tgz`))).toBe(true);
    const refPath = join(dir, 'v1', 'refs', sha256Hex(Buffer.from('oci://reg.example.com|x|1.0.0', 'utf8')));
    expect(existsSync(refPath)).toBe(true);
    expect((await readFile(refPath, 'utf8')).trim()).toBe(digest);
  });

  it('hits the cache on second call (no fetch)', async () => {
    const bytes = new Uint8Array([0x1f, 0x8b, 0x42, 0x02]);
    const fetchMock = vi.fn(async (url: string | URL) => {
      const u = url.toString();
      if (u.includes('/manifests/')) {
        return new Response(
          JSON.stringify({
            layers: [
              {
                mediaType: 'application/vnd.cncf.helm.chart.content.v1.tar+gzip',
                digest: 'sha256:y',
                size: bytes.byteLength,
              },
            ],
          }),
          { status: 200 },
        );
      }
      return new Response(bytes, { status: 200 });
    });
    globalThis.fetch = fetchMock as unknown as typeof fetch;
    process.env.AKUA_CACHE_DIR = dir;

    await pullChartCached('oci://reg.example.com/x:1.0.0');
    fetchMock.mockClear();
    const secondCall = await pullChartCached('oci://reg.example.com/x:1.0.0');
    expect(secondCall).toEqual(bytes);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('AKUA_NO_CACHE=1 bypasses cache', async () => {
    const bytes = new Uint8Array([0x1f, 0x8b, 0x42, 0x03]);
    const fetchMock = vi.fn(async (url: string | URL) => {
      const u = url.toString();
      if (u.includes('/manifests/')) {
        return new Response(
          JSON.stringify({
            layers: [
              {
                mediaType: 'application/vnd.cncf.helm.chart.content.v1.tar+gzip',
                digest: 'sha256:z',
                size: bytes.byteLength,
              },
            ],
          }),
          { status: 200 },
        );
      }
      return new Response(bytes, { status: 200 });
    });
    globalThis.fetch = fetchMock as unknown as typeof fetch;
    process.env.AKUA_CACHE_DIR = dir;
    process.env.AKUA_NO_CACHE = '1';

    await pullChartCached('oci://reg.example.com/x:1.0.0');
    const callsAfterFirst = fetchMock.mock.calls.length;
    fetchMock.mockClear();
    await pullChartCached('oci://reg.example.com/x:1.0.0');
    // Second call should hit network again (same number of fetches as first).
    expect(fetchMock.mock.calls.length).toBe(callsAfterFirst);
  });

  it('falls through to live fetch when cached blob is corrupt', async () => {
    const bytes = new Uint8Array([0x1f, 0x8b, 0x42, 0x04]);
    // Pre-populate a ref pointing at a blob whose contents don't hash to the ref value.
    process.env.AKUA_CACHE_DIR = dir;
    const key = 'oci://reg.example.com|x|1.0.0';
    const { mkdir } = await import('node:fs/promises');
    await mkdir(join(dir, 'v1', 'refs'), { recursive: true });
    await mkdir(join(dir, 'v1', 'blobs'), { recursive: true });
    const fakeDigest = 'a'.repeat(64);
    await writeFile(join(dir, 'v1', 'refs', sha256Hex(Buffer.from(key, 'utf8'))), fakeDigest);
    await writeFile(join(dir, 'v1', 'blobs', `${fakeDigest}.tgz`), 'garbage');

    globalThis.fetch = (async (url: string | URL) => {
      const u = url.toString();
      if (u.includes('/manifests/')) {
        return new Response(
          JSON.stringify({
            layers: [
              {
                mediaType: 'application/vnd.cncf.helm.chart.content.v1.tar+gzip',
                digest: 'sha256:q',
                size: bytes.byteLength,
              },
            ],
          }),
          { status: 200 },
        );
      }
      return new Response(bytes, { status: 200 });
    }) as unknown as typeof fetch;

    const got = await pullChartCached('oci://reg.example.com/x:1.0.0');
    expect(got).toEqual(bytes);
  });
});
