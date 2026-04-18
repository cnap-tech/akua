/* tslint:disable */
/* eslint-disable */

/**
 * Apply schema transforms (slugify, template substitution) to user inputs.
 *
 * `fields` is the output of `extractInstallFields`; `inputs` is an object
 * mapping dot-paths to string values. Returns resolved values nested by path.
 */
export function applyInstallTransforms(fields: any, inputs: any): any;

/**
 * Build `.akua/metadata.yaml` provenance. Caller supplies `buildTime`
 * as an RFC 3339 string — JS sees `SystemTime::now()` panic in WASM,
 * so the timestamp is computed host-side (SDK reads `SOURCE_DATE_EPOCH`
 * on Node, falls back to `new Date().toISOString()`).
 */
export function buildMetadata(sources: any, fields: any, build_time: string): any;

/**
 * Build an umbrella Helm chart from a set of sources. Returns
 * `{ chartYaml, values }`.
 */
export function buildUmbrellaChart(name: string, version: string, sources: any): any;

/**
 * Extract `x-user-input` / `x-install` fields from a JSON Schema.
 */
export function extractInstallFields(schema: any): any;

/**
 * Deterministic short alias suffix (djb2 + base36). Used for chart aliases.
 */
export function hashToSuffix(input: string, length: number): string;

export function init(): void;

/**
 * Merge values from multiple sources into a single object, nested by alias.
 */
export function mergeSourceValues(sources: any): any;

/**
 * Merge JSON Schemas from multiple sources into one umbrella schema.
 *
 * Input: array of `{ source, schema? }`. Output: a single
 * `type: object` schema where each source's schema nests under its
 * deterministic alias (same alias the values use). Sources without a
 * schema are skipped. Used by the install wizard to show one combined
 * form for a multi-source package.
 */
export function mergeValuesSchemas(sources: any): any;

/**
 * Validate a values.schema.json structurally. Returns the error message,
 * or `null` if the schema is valid.
 */
export function validateValuesSchema(schema: any): string | undefined;
