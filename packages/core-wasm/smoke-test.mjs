// Smoke test for akua-wasm bindings.
// Run with: node packages/core-wasm/smoke-test.mjs
import * as akua from './akua_wasm.js';

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

// 1. hash_to_suffix — deterministic
console.log('hashToSuffix:', akua.hashToSuffix('thsrc_abc123', 4));

// 2. validateValuesSchema — should be null for valid
console.log('validateValuesSchema:', akua.validateValuesSchema(schema) ?? 'ok');

// 3. extractInstallFields
const fields = akua.extractInstallFields(schema);
console.log('extractInstallFields:', JSON.stringify(fields, null, 2));

// 4. applyInstallTransforms
const resolved = akua.applyInstallTransforms(fields, {
  'config.adminEmail': 'admin@example.com',
  'httpRoute.hostname': 'My App!',
});
console.log('applyInstallTransforms:', JSON.stringify(resolved, null, 2));

// 5. mergeValuesSchemas — combine two per-source schemas into one
const perSourceSchemas = [
  {
    source: {
      name: 'web',
      helm: { repo: 'https://charts.example.com', chart: 'nginx', version: '1.0.0' },
    },
    schema: {
      type: 'object',
      properties: { replicaCount: { type: 'integer' } },
    },
  },
  {
    source: {
      name: 'cache',
      helm: { repo: 'oci://ghcr.io/org/redis', version: '7.0.0' },
    },
    schema: {
      type: 'object',
      properties: { port: { type: 'integer' } },
    },
  },
];
const mergedSchema = akua.mergeValuesSchemas(perSourceSchemas);
console.log('mergeValuesSchemas:', JSON.stringify(mergedSchema, null, 2));

// 6. buildUmbrellaChart
const sources = [
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
const umbrella = akua.buildUmbrellaChart('demo', '0.1.0', sources);
console.log('buildUmbrellaChart:', JSON.stringify(umbrella, null, 2));
