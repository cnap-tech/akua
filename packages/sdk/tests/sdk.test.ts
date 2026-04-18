import { describe, it, expect, beforeAll } from 'vitest';

import {
  init,
  buildUmbrellaChart,
  extractInstallFields,
  applyInstallTransforms,
  validateValuesSchema,
  mergeSourceValues,
  mergeValuesSchemas,
  hashToSuffix,
  type ExtractedInstallField,
  type Source,
  type SourceWithSchema,
} from '../src/index.node.ts';

beforeAll(async () => {
  await init();
});

describe('hashToSuffix', () => {
  it('is deterministic and respects length', () => {
    expect(hashToSuffix('thsrc_abc123', 4)).toHaveLength(4);
    expect(hashToSuffix('thsrc_abc123', 4)).toBe(hashToSuffix('thsrc_abc123', 4));
  });
});

describe('buildUmbrellaChart', () => {
  it('returns a Chart.yaml + merged values for a single helm source', () => {
    const sources: Source[] = [
      {
        name: 'app',
        helm: {
          repo: 'https://charts.bitnami.com/bitnami',
          chart: 'nginx',
          version: '18.1.0',
        },
        values: { replicaCount: 1 },
      },
    ];
    const umbrella = buildUmbrellaChart('pkg', '0.1.0', sources);

    expect(umbrella.chartYaml.apiVersion).toBe('v2');
    expect(umbrella.chartYaml.name).toBe('pkg');
    expect(umbrella.chartYaml.dependencies?.[0]?.alias).toBe('app');
    expect(umbrella.values).toEqual({ app: { replicaCount: 1 } });
  });

  it('uses source name as alias regardless of chart name', () => {
    const umbrella = buildUmbrellaChart('pkg', '0.1.0', [
      {
        name: 'web',
        helm: { repo: 'https://charts.example.com', chart: 'nginx', version: '1.0.0' },
        values: { port: 80 },
      },
    ]);
    expect(umbrella.chartYaml.dependencies?.[0]?.alias).toBe('web');
    expect(umbrella.values).toEqual({ web: { port: 80 } });
  });
});

describe('extractInstallFields + applyInstallTransforms', () => {
  const schema = {
    type: 'object',
    properties: {
      config: {
        type: 'object',
        properties: {
          adminEmail: {
            type: 'string',
            'x-user-input': { order: 10 },
          },
        },
        required: ['adminEmail'],
      },
      httpRoute: {
        type: 'object',
        properties: {
          hostname: {
            type: 'string',
            'x-user-input': { order: 20 },
            'x-input': { cel: "slugify(value) + '.apps.example.com'" },
          },
        },
        required: ['hostname'],
      },
    },
    required: ['config', 'httpRoute'],
  };

  it('extracts all x-user-input leaf fields sorted by order', () => {
    const fields = extractInstallFields(schema);
    expect(fields.map((f) => f.path)).toEqual(['config.adminEmail', 'httpRoute.hostname']);
    expect(fields[0]!.required).toBe(true);
  });

  it('applies CEL transforms and passes through unmarked values', () => {
    const fields = extractInstallFields(schema);
    const resolved = applyInstallTransforms(fields, {
      'config.adminEmail': 'admin@example.com',
      'httpRoute.hostname': 'My App!',
    });
    expect(resolved).toEqual({
      config: { adminEmail: 'admin@example.com' },
      httpRoute: { hostname: 'my-app.apps.example.com' },
    });
  });

  it('surfaces missing-required-field errors through a thrown JsValue', () => {
    const fields = extractInstallFields(schema);
    expect(() => applyInstallTransforms(fields, { 'config.adminEmail': '' })).toThrow();
  });

  it('preserves the raw x-input bag so consumers can read non-Akua transform keys', () => {
    const fields = extractInstallFields({
      type: 'object',
      properties: {
        region: {
          type: 'string',
          'x-user-input': true,
          'x-input': { jsonnet: 'std.asciiLower(value)', custom: 42 },
        },
      },
      required: ['region'],
    });
    const input = (fields[0]!.schema as Record<string, unknown>)['x-input'] as Record<
      string,
      unknown
    >;
    expect(input).toEqual({ jsonnet: 'std.asciiLower(value)', custom: 42 });
  });
});

describe('validateValuesSchema', () => {
  it('returns null for a valid schema', () => {
    expect(validateValuesSchema({ type: 'object', properties: {} })).toBeNull();
  });

  it('rejects a non-object root', () => {
    expect(validateValuesSchema({ type: 'string' })).toBeTruthy();
  });
});

describe('merge helpers', () => {
  const sources: Source[] = [
    {
      name: 'primary',
      helm: { repo: 'https://x', chart: 'redis', version: '7.0.0' },
      values: { port: 6379 },
    },
    {
      name: 'replica',
      helm: { repo: 'https://x', chart: 'redis', version: '7.0.0' },
      values: { port: 6380 },
    },
  ];

  it('mergeSourceValues nests by source name', () => {
    const merged = mergeSourceValues(sources);
    expect(merged).toEqual({
      primary: { port: 6379 },
      replica: { port: 6380 },
    });
  });

  it('mergeValuesSchemas combines per-source schemas under aliases', () => {
    const withSchemas: SourceWithSchema[] = sources.map((s) => ({
      source: s,
      schema: {
        type: 'object',
        properties: { port: { type: 'integer' } },
      },
    }));
    const merged = mergeValuesSchemas(withSchemas) as {
      type: string;
      properties: Record<string, unknown>;
    };
    expect(merged.type).toBe('object');
    expect(Object.keys(merged.properties)).toEqual(['primary', 'replica']);
  });
});

describe('init', () => {
  it('is idempotent', async () => {
    await init();
    await init();
    await init();
  });
});

// Type-level regression test: the public Source/HelmBlock/ExtractedInstallField
// shapes must stay assignment-compatible with what the WASM expects.
// Doesn't execute — compiler enforces.
function _typeCheck() {
  const _s: Source = { name: 'a', helm: { repo: 'x', chart: 'y', version: '1.0.0' } };
  const _f: ExtractedInstallField = { path: 'p', schema: {}, required: false };
  return [_s, _f];
}
