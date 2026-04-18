/**
 * Public Akua SDK surface. All functions require `init()` to run first;
 * we re-export `init` from the env-specific entry (Node or browser) so
 * consumers never have to think about which one to call.
 *
 * Usage:
 *
 * ```ts
 * import { init, buildUmbrellaChart, extractInstallFields } from '@akua/sdk';
 *
 * await init();
 * const umbrella = buildUmbrellaChart('my-pkg', '0.1.0', [
 *   { name: 'app', helm: { repo: 'https://charts.bitnami.com/bitnami', chart: 'nginx', version: '18.1.0' } },
 * ]);
 * ```
 */

import { wasm } from './wasm.js';
import type {
  ExtractedInstallField,
  JsonSchema,
  ResolvedValues,
  Source,
  SourceWithSchema,
  UmbrellaChart,
} from './types.js';

export { pullChart, parseOciRef, parseBearerChallenge, OciPullError } from './oci.js';
export type { OciAuth, OciCredentials, PullChartOptions } from './oci.js';

export {
  unpackTgz,
  streamTgzEntries,
  packTgz,
  packTgzStream,
  inspectChartBytes,
  TarError,
} from './tar.js';
export type { TgzInput, PackEntries } from './tar.js';

export { packChart, packChartStream, dumpYaml } from './chart.js';

export type {
  ChartDependency,
  ChartYaml,
  ExtractedInstallField,
  HelmBlock,
  HelmfileBlock,
  JsonSchema,
  KclBlock,
  ResolvedValues,
  Source,
  SourceWithSchema,
  UmbrellaChart,
} from './types.js';

/** Deterministic short alias suffix (djb2 + base36). */
export function hashToSuffix(input: string, length: number): string {
  return wasm().hashToSuffix(input, length);
}

/**
 * Walk a JSON Schema and return all `x-user-input`-marked leaf fields,
 * sorted by `x-user-input.order`.
 */
export function extractInstallFields(schema: JsonSchema): ExtractedInstallField[] {
  return wasm().extractInstallFields(schema) as ExtractedInstallField[];
}

/**
 * Apply `x-input` transforms (CEL expressions) to user-provided inputs
 * using the fields extracted from a schema. Returns a nested object of
 * resolved values.
 *
 * @throws when a required field is missing or a CEL expression errors.
 */
export function applyInstallTransforms(
  fields: ExtractedInstallField[],
  inputs: Record<string, string>,
): ResolvedValues {
  return wasm().applyInstallTransforms(fields, inputs) as ResolvedValues;
}

/**
 * Structurally validate a `values.schema.json`. Returns `null` when
 * valid, otherwise a human-readable error message.
 */
export function validateValuesSchema(schema: JsonSchema): string | null {
  return wasm().validateValuesSchema(schema) ?? null;
}

/** Merge the `values` blocks from multiple sources, nested under each source's alias. */
export function mergeSourceValues(sources: Source[]): Record<string, unknown> {
  return wasm().mergeSourceValues(sources) as Record<string, unknown>;
}

/**
 * Merge JSON Schemas from multiple sources into one umbrella schema
 * (each source's schema nests under its alias). Use for install
 * wizards that render a combined form.
 */
export function mergeValuesSchemas(sources: SourceWithSchema[]): JsonSchema {
  return wasm().mergeValuesSchemas(sources) as JsonSchema;
}

/** Assemble an umbrella Helm chart from a set of sources. */
export function buildUmbrellaChart(
  name: string,
  version: string,
  sources: Source[],
): UmbrellaChart {
  return wasm().buildUmbrellaChart(name, version, sources) as UmbrellaChart;
}
