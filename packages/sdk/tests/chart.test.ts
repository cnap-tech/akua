import { describe, it, expect, beforeAll, afterEach } from 'vitest';

import {
  init,
  buildMetadata,
  dependencyToOciRef,
  packChart,
  AkuaError,
  type AkuaMetadata,
  type ChartDependency,
  type JsonSchema,
  type Source,
  type UmbrellaChart,
} from '../src/index.node.ts';
import { OciPullError } from '../src/oci.ts';
import { TarError, streamTgzEntries } from '../src/tar.ts';

beforeAll(async () => {
  await init();
});

function makeUmbrella(overrides: Partial<UmbrellaChart> = {}): UmbrellaChart {
  return {
    chartYaml: {
      apiVersion: 'v2',
      name: 'pkg',
      version: '0.1.0',
      type: 'application',
      dependencies: [],
      ...overrides.chartYaml,
    },
    values: overrides.values ?? {},
  };
}

async function readEntries(bytes: Uint8Array): Promise<Record<string, Uint8Array>> {
  const out: Record<string, Uint8Array> = {};
  for await (const entry of streamTgzEntries(bytes)) {
    out[entry.path] = entry.bytes;
  }
  return out;
}

describe('dependencyToOciRef', () => {
  it('joins repository + name + version for oci:// deps', () => {
    const dep: ChartDependency = {
      name: 'bar',
      version: '1.0.0',
      repository: 'oci://ghcr.io/foo',
    };
    expect(dependencyToOciRef(dep)).toBe('oci://ghcr.io/foo/bar:1.0.0');
  });

  it('returns null for non-oci repositories', () => {
    expect(
      dependencyToOciRef({ name: 'x', version: '1', repository: 'https://example.com' }),
    ).toBeNull();
    expect(
      dependencyToOciRef({ name: 'x', version: '1', repository: 'file:///tmp/x' }),
    ).toBeNull();
    expect(dependencyToOciRef({ name: 'x', version: '1', repository: '' })).toBeNull();
  });
});

describe('packChart — file:// scrubbing', () => {
  it('clears repository when it starts with file://', async () => {
    const umbrella = makeUmbrella({
      chartYaml: {
        apiVersion: 'v2',
        name: 'pkg',
        version: '0.1.0',
        dependencies: [
          { name: 'sub', version: '1.0.0', repository: 'file:///tmp/sub', alias: 'sub' },
          { name: 'other', version: '2.0.0', repository: 'oci://ghcr.io/x' },
        ],
      },
    });
    const tgz = await packChart(
      umbrella,
      new Map([['sub', new Uint8Array([0x1f, 0x8b])]]),
    );
    const entries = await readEntries(tgz);
    const chartYaml = new TextDecoder().decode(entries['pkg/Chart.yaml']);
    expect(chartYaml).toContain('repository: ""');
    expect(chartYaml).toContain('repository: "oci://ghcr.io/x"');
  });

  it('leaves non-file:// repositories untouched', async () => {
    const umbrella = makeUmbrella({
      chartYaml: {
        apiVersion: 'v2',
        name: 'pkg',
        version: '0.1.0',
        dependencies: [
          { name: 'sub', version: '1.0.0', repository: 'oci://ghcr.io/foo', alias: 'sub' },
        ],
      },
    });
    const tgz = await packChart(
      umbrella,
      new Map([['sub', new Uint8Array([0x1f, 0x8b])]]),
    );
    const entries = await readEntries(tgz);
    const chartYaml = new TextDecoder().decode(entries['pkg/Chart.yaml']);
    expect(chartYaml).toContain('repository: "oci://ghcr.io/foo"');
  });
});

describe('packChart — optional files', () => {
  it('emits values.schema.json when provided', async () => {
    const schema: JsonSchema = { type: 'object', properties: { replicas: { type: 'integer' } } };
    const tgz = await packChart(makeUmbrella(), new Map(), { valuesSchema: schema });
    const entries = await readEntries(tgz);
    expect(entries['pkg/values.schema.json']).toBeDefined();
    const parsed = JSON.parse(new TextDecoder().decode(entries['pkg/values.schema.json']));
    expect(parsed).toEqual(schema);
  });

  it('emits .akua/metadata.yaml when metadata provided', async () => {
    const meta: AkuaMetadata = {
      akuaVersion: '0.1.0',
      buildTime: '2026-04-18T00:00:00Z',
      package: { name: 'pkg', version: '0.1.0' },
      sources: [{ name: 'app', engine: 'helm', ref: 'oci://ghcr.io/foo/bar:1.0.0' }],
    };
    const tgz = await packChart(makeUmbrella(), new Map(), { metadata: meta });
    const entries = await readEntries(tgz);
    const metadataYaml = new TextDecoder().decode(entries['pkg/.akua/metadata.yaml']);
    expect(metadataYaml).toContain('akuaVersion: 0.1.0');
    expect(metadataYaml).toContain('buildTime:');
  });

  it('omits optional entries when not provided', async () => {
    const tgz = await packChart(makeUmbrella(), new Map());
    const entries = await readEntries(tgz);
    expect(entries['pkg/values.schema.json']).toBeUndefined();
    expect(entries['pkg/.akua/metadata.yaml']).toBeUndefined();
  });
});

describe('packChart — AbortSignal', () => {
  it('rejects when signal is already aborted', async () => {
    const controller = new AbortController();
    controller.abort(new Error('user cancelled'));
    await expect(
      packChart(makeUmbrella(), new Map(), { signal: controller.signal }),
    ).rejects.toThrow();
  });

  it('rejects mid-stream when signal fires during subchart iteration', async () => {
    const controller = new AbortController();
    const subcharts = async function* (): AsyncGenerator<readonly [string, Uint8Array]> {
      yield ['a', new Uint8Array([1])] as const;
      controller.abort(new Error('aborted'));
      yield ['b', new Uint8Array([2])] as const;
    };
    const umbrella = makeUmbrella({
      chartYaml: {
        apiVersion: 'v2',
        name: 'pkg',
        version: '0.1.0',
        dependencies: [
          { name: 'a', version: '1.0.0', repository: '', alias: 'a' },
          { name: 'b', version: '1.0.0', repository: '', alias: 'b' },
        ],
      },
    });
    await expect(packChart(umbrella, subcharts(), { signal: controller.signal })).rejects.toThrow();
  });
});

describe('buildMetadata', () => {
  const makeSources = (): Source[] => [
    {
      name: 'app',
      helm: { repo: 'https://charts.example.com', chart: 'nginx', version: '1.0.0' },
    },
  ];

  it('emits akua version + sources + empty transforms', () => {
    const meta = buildMetadata(makeSources());
    expect(meta.akua.version).toMatch(/^\d/);
    expect(meta.akua.buildTime).toMatch(/^\d{4}-\d{2}-\d{2}T/);
    expect(meta.sources).toHaveLength(1);
    expect(meta.sources?.[0]?.name).toBe('app');
    expect(meta.sources?.[0]?.engine).toBe('helm');
    expect(meta.sources?.[0]?.origin).toBe('https://charts.example.com/nginx');
    expect(meta.transforms).toBeUndefined();
  });

  it('honours SOURCE_DATE_EPOCH (env) for reproducible buildTime', () => {
    const prior = process.env.SOURCE_DATE_EPOCH;
    process.env.SOURCE_DATE_EPOCH = '1700000000';
    try {
      const meta = buildMetadata(makeSources());
      expect(meta.akua.buildTime).toBe('2023-11-14T22:13:20.000Z');
    } finally {
      if (prior === undefined) delete process.env.SOURCE_DATE_EPOCH;
      else process.env.SOURCE_DATE_EPOCH = prior;
    }
  });

  it('accepts an explicit buildTime override', () => {
    const meta = buildMetadata(makeSources(), [], { buildTime: '2026-01-01T00:00:00Z' });
    expect(meta.akua.buildTime).toBe('2026-01-01T00:00:00Z');
  });
});

describe('pullChartStream (OCI path)', () => {
  const realFetch = globalThis.fetch;
  afterEach(() => {
    globalThis.fetch = realFetch;
  });

  it('returns a ReadableStream piping straight through without buffering', async () => {
    const { pullChartStream } = await import('../src/oci.ts');
    const layerBytes = new Uint8Array([0x1f, 0x8b, 0x08, 0x00]);
    globalThis.fetch = (async (url: string | URL) => {
      const u = url.toString();
      if (u.endsWith('/manifests/1.0.0')) {
        return new Response(
          JSON.stringify({
            layers: [
              { mediaType: 'application/vnd.cncf.helm.chart.content.v1.tar+gzip', digest: 'sha256:x', size: layerBytes.byteLength },
            ],
          }),
          { status: 200 },
        );
      }
      if (u.includes('/blobs/')) return new Response(layerBytes, { status: 200 });
      return new Response(null, { status: 404 });
    }) as unknown as typeof fetch;

    const stream = await pullChartStream('oci://reg.example.com/pkg/chart:1.0.0');
    expect(stream).toBeInstanceOf(ReadableStream);
    const buf = new Uint8Array(await new Response(stream).arrayBuffer());
    expect(buf).toEqual(layerBytes);
  });
});

describe('error class hierarchy', () => {
  it('OciPullError is an AkuaError', () => {
    const err = new OciPullError('boom');
    expect(err).toBeInstanceOf(AkuaError);
    expect(err).toBeInstanceOf(Error);
    expect(err.name).toBe('OciPullError');
  });

  it('TarError is an AkuaError', () => {
    const err = new TarError('bad tar');
    expect(err).toBeInstanceOf(AkuaError);
    expect(err).toBeInstanceOf(Error);
    expect(err.name).toBe('TarError');
  });
});
