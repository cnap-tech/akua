import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  findIndexEntry,
  HelmHttpError,
  parseHelmHttpRef,
  pullHelmHttpChart,
} from '../src/helm-http.ts';
import { AkuaError } from '../src/errors.ts';

describe('parseHelmHttpRef', () => {
  it('parses a standard https ref', () => {
    expect(parseHelmHttpRef('https://charts.example.com/bitnami/nginx:18.1.0')).toEqual({
      repo: 'https://charts.example.com/bitnami',
      chart: 'nginx',
      version: '18.1.0',
    });
  });

  it('parses an http ref', () => {
    expect(parseHelmHttpRef('http://localhost:8080/repo/chart:0.1.0')).toEqual({
      repo: 'http://localhost:8080/repo',
      chart: 'chart',
      version: '0.1.0',
    });
  });

  it('rejects non-http schemes', () => {
    expect(() => parseHelmHttpRef('oci://foo/bar:1.0.0')).toThrow(HelmHttpError);
  });

  it('rejects missing version', () => {
    expect(() => parseHelmHttpRef('https://charts.example.com/foo/bar')).toThrow(HelmHttpError);
  });
});

describe('findIndexEntry', () => {
  const SAMPLE_INDEX = `apiVersion: v1
entries:
  nginx:
    - apiVersion: v2
      name: nginx
      version: 18.1.0
      digest: sha256:abcdef
      urls:
        - https://charts.example.com/nginx-18.1.0.tgz
    - apiVersion: v2
      name: nginx
      version: 18.0.0
      digest: sha256:111
      urls:
        - nginx-18.0.0.tgz
  postgres:
    - apiVersion: v2
      name: postgres
      version: 15.0.0
      digest: sha256:zzz
      urls:
        - https://charts.example.com/postgres-15.0.0.tgz
generated: "2025-01-01T00:00:00Z"
`;

  it('finds the requested version by chart name', () => {
    expect(findIndexEntry(SAMPLE_INDEX, 'nginx', '18.1.0')).toEqual({
      version: '18.1.0',
      digest: 'sha256:abcdef',
      urls: ['https://charts.example.com/nginx-18.1.0.tgz'],
    });
  });

  it('skips over other charts to find the target', () => {
    expect(findIndexEntry(SAMPLE_INDEX, 'postgres', '15.0.0')?.digest).toBe('sha256:zzz');
  });

  it('returns null for unknown chart', () => {
    expect(findIndexEntry(SAMPLE_INDEX, 'redis', '1.0.0')).toBeNull();
  });

  it('returns null for unknown version of a known chart', () => {
    expect(findIndexEntry(SAMPLE_INDEX, 'nginx', '99.0.0')).toBeNull();
  });

  it('returns relative urls as-is (caller resolves against repo)', () => {
    expect(findIndexEntry(SAMPLE_INDEX, 'nginx', '18.0.0')?.urls).toEqual(['nginx-18.0.0.tgz']);
  });

  it('handles quoted scalars', () => {
    const indexWithQuotes = `entries:
  nginx:
    - version: "18.1.0"
      digest: "sha256:qqq"
      urls:
        - "https://example.com/nginx.tgz"
`;
    expect(findIndexEntry(indexWithQuotes, 'nginx', '18.1.0')).toEqual({
      version: '18.1.0',
      digest: 'sha256:qqq',
      urls: ['https://example.com/nginx.tgz'],
    });
  });
});

describe('pullHelmHttpChart', () => {
  const realFetch = globalThis.fetch;
  afterEach(() => {
    globalThis.fetch = realFetch;
  });

  it('fetches index, resolves url, validates digest, returns bytes', async () => {
    const chartBytes = new Uint8Array([0x1f, 0x8b, 0x01, 0x02]);
    // Pre-compute the expected digest for chartBytes.
    const hashBuf = await crypto.subtle.digest('SHA-256', chartBytes);
    const hex = Array.from(new Uint8Array(hashBuf))
      .map((b) => b.toString(16).padStart(2, '0'))
      .join('');
    const index = `entries:
  nginx:
    - version: "1.0.0"
      digest: "sha256:${hex}"
      urls:
        - https://charts.example.com/nginx-1.0.0.tgz
`;
    const fetchMock = vi.fn(async (url: string | URL) => {
      const u = url.toString();
      if (u.endsWith('/index.yaml')) return new Response(index, { status: 200 });
      if (u.endsWith('/nginx-1.0.0.tgz')) return new Response(chartBytes, { status: 200 });
      return new Response('not found', { status: 404 });
    });
    globalThis.fetch = fetchMock as unknown as typeof fetch;

    const bytes = await pullHelmHttpChart('https://charts.example.com/nginx:1.0.0');
    expect(bytes).toEqual(chartBytes);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it('rejects when digest mismatch', async () => {
    const index = `entries:
  x:
    - version: "1.0.0"
      digest: "sha256:0000000000000000000000000000000000000000000000000000000000000000"
      urls:
        - https://charts.example.com/x-1.0.0.tgz
`;
    globalThis.fetch = (async (url: string | URL) => {
      const u = url.toString();
      if (u.endsWith('/index.yaml')) return new Response(index, { status: 200 });
      return new Response(new Uint8Array([1, 2, 3]), { status: 200 });
    }) as unknown as typeof fetch;

    await expect(pullHelmHttpChart('https://charts.example.com/x:1.0.0')).rejects.toThrow(
      /digest mismatch/,
    );
  });

  it('throws HelmHttpError (an AkuaError) on 404 index', async () => {
    globalThis.fetch = (async () => new Response('nope', { status: 404 })) as unknown as typeof fetch;
    try {
      await pullHelmHttpChart('https://charts.example.com/x:1.0.0');
      expect.fail('should throw');
    } catch (err) {
      expect(err).toBeInstanceOf(HelmHttpError);
      expect(err).toBeInstanceOf(AkuaError);
    }
  });
});
