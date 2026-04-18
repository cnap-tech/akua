/**
 * Public TypeScript types for the Akua SDK. These mirror `akua-core`'s
 * v1alpha1 data structures — `Source`, engine blocks, schema types —
 * hand-written so consumers get real IntelliSense instead of the
 * wasm-bindgen-emitted `any`/`JsValue`.
 *
 * Authoritative spec: `docs/design-package-yaml-v1.md` + `docs/spec-markers.md`.
 */

/** A single source under `sources:` in `package.yaml`. Exactly one engine block must be set. */
export interface Source {
  name: string;
  helm?: HelmBlock;
  kcl?: KclBlock;
  helmfile?: HelmfileBlock;
  /** Default values for the source. Deep-merged at render time. */
  values?: Record<string, unknown>;
}

export interface HelmBlock {
  /** `https://…` for HTTP Helm repos, `oci://host/path` for OCI. */
  repo: string;
  /** Chart name. Optional when `repo` already terminates at the chart (`oci://…/<chart>`). */
  chart?: string;
  /** Exact version pin. No ranges. */
  version: string;
}

export interface KclBlock {
  entrypoint: string;
  version: string;
}

export interface HelmfileBlock {
  path: string;
  version: string;
}

/**
 * One leaf field extracted from a JSON Schema via `x-user-input`. The
 * `schema` property is the raw JSON Schema node, including any `x-input`
 * transform bag — consumers read extensions directly (e.g.
 * `field.schema['x-input']?.cel`) rather than through privileged struct
 * fields.
 */
export interface ExtractedInstallField {
  path: string;
  schema: JsonSchema;
  required: boolean;
}

/** Opaque JSON Schema value. */
export type JsonSchema = Record<string, unknown>;

/** Output of `buildUmbrella`. */
export interface UmbrellaChart {
  chartYaml: ChartYaml;
  values: Record<string, unknown>;
}

export interface ChartYaml {
  apiVersion: string;
  name: string;
  version: string;
  description?: string;
  type?: string;
  appVersion?: string;
  keywords?: string[];
  home?: string;
  sources?: string[];
  icon?: string;
  annotations?: Record<string, string>;
  maintainers?: { name?: string; email?: string; url?: string }[];
  dependencies?: ChartDependency[];
}

export interface ChartDependency {
  name: string;
  version: string;
  repository: string;
  alias?: string;
  condition?: string;
}

/** Pairing used by `mergeValuesSchemas`. */
export interface SourceWithSchema {
  source: Source;
  schema?: JsonSchema;
}

/** Resolved values from `applyInstallTransforms`. */
export type ResolvedValues = Record<string, unknown>;

/**
 * Provenance sidecar written to `.akua/metadata.yaml`. Mirrors
 * `akua-core::metadata::AkuaMetadata`.
 */
export interface AkuaMetadata {
  akua: {
    version: string;
    buildTime: string;
  };
  sources?: AkuaMetadataSource[];
  transforms?: AkuaMetadataTransform[];
}

export interface AkuaMetadataSource {
  name: string;
  engine: string;
  origin: string;
  version: string;
  alias?: string;
}

export interface AkuaMetadataTransform {
  field: string;
  required: boolean;
  input?: Record<string, unknown>;
}
