/**
 * High-level chart assembly — compose `buildUmbrellaChart`'s in-memory
 * umbrella tree + fetched subchart tarballs into a single packaged
 * `.tgz`, without shelling out to `akua package` or `helm package`.
 *
 * Pattern:
 *
 * ```ts
 * const sources: Source[] = [...];
 * const umbrella = buildUmbrellaChart('my-pkg', '0.1.0', sources);
 *
 * // Pull each remote subchart. `pullChart` is SDK-native; file://
 * // engines materialise bytes elsewhere (kcl/helmfile output).
 * const subcharts = new Map<string, Uint8Array>();
 * for (const dep of umbrella.chartYaml.dependencies ?? []) {
 *   const bytes = await pullChart(`${dep.repository}/${dep.name}:${dep.version}`);
 *   subcharts.set(dep.alias ?? dep.name, bytes);
 * }
 *
 * const tgz = await packChart(umbrella, subcharts);
 * // → feed to OCI push, write to disk, `helm install ./pkg.tgz`, etc.
 * ```
 *
 * Why the caller provides `subcharts`: network I/O + auth stay in the
 * caller's hands (they can use their existing registry auth, cache, or
 * our own `pullChart`). The SDK is strict about being disk-free and
 * network-free outside the explicit `pullChart` function.
 */

import { packTgzStream } from './tar.js';
import type { UmbrellaChart } from './types.js';

// Small subset of js-yaml's API we need — hand-rolled dumper below.
// We avoid a js-yaml dep so the SDK stays small.

/**
 * Pack an assembled umbrella chart + subchart bytes into a single
 * Helm-compatible `.tgz`. The output matches what `helm package` emits
 * for the same chart layout.
 */
export async function packChart(
  umbrella: UmbrellaChart,
  subcharts: Map<string, Uint8Array>,
): Promise<Uint8Array> {
  return new Uint8Array(await new Response(packChartStream(umbrella, subcharts)).arrayBuffer());
}

/**
 * Streaming variant of [`packChart`]. Returns a `ReadableStream` that
 * produces the `.tgz` on demand. Pipe to disk (Node `fs.createWriteStream`
 * via `Readable.fromWeb`), to an OCI push, or to `fetch()` as a body —
 * the chart is never fully held in memory. `subcharts` can be sync or
 * async iterable so the caller can stream pulls straight through.
 */
export function packChartStream(
  umbrella: UmbrellaChart,
  subcharts:
    | Map<string, Uint8Array>
    | Iterable<readonly [string, Uint8Array]>
    | AsyncIterable<readonly [string, Uint8Array]>,
): ReadableStream<Uint8Array> {
  const chartName = umbrella.chartYaml.name;
  const chartYamlBytes = textEncode(
    dumpYaml(umbrella.chartYaml as unknown as Record<string, unknown>),
  );
  const valuesYamlBytes = textEncode(dumpYaml(umbrella.values));

  async function* entries(): AsyncGenerator<readonly [string, Uint8Array]> {
    yield ['Chart.yaml', chartYamlBytes];
    yield ['values.yaml', valuesYamlBytes];
    const iter =
      Symbol.asyncIterator in (subcharts as object)
        ? (subcharts as AsyncIterable<readonly [string, Uint8Array]>)
        : (subcharts as Iterable<readonly [string, Uint8Array]>);
    for await (const [alias, bytes] of iter as AsyncIterable<readonly [string, Uint8Array]>) {
      if (!alias) continue;
      yield [`charts/${alias}-${findVersion(umbrella, alias)}.tgz`, bytes];
    }
  }

  return packTgzStream(chartName, entries());
}

function findVersion(umbrella: UmbrellaChart, alias: string): string {
  const dep = umbrella.chartYaml.dependencies?.find(
    (d) => d.alias === alias || d.name === alias,
  );
  return dep?.version ?? '0.0.0';
}

// ---------------------------------------------------------------------------
// Minimal YAML dumper (mirrors what Chart.yaml / values.yaml need)
// ---------------------------------------------------------------------------

/** Dump a JS value as YAML. Covers the subset Helm / Akua emit. */
export function dumpYaml(value: unknown, indent = 0): string {
  if (value === null || value === undefined) return 'null\n';
  if (typeof value === 'boolean') return `${value}\n`;
  if (typeof value === 'number') return `${value}\n`;
  if (typeof value === 'string') return `${quoteIfNeeded(value)}\n`;
  if (Array.isArray(value)) {
    if (value.length === 0) return '[]\n';
    let out = '';
    for (const item of value) {
      out += `${' '.repeat(indent)}- ${dumpInline(item, indent + 2)}`;
    }
    return out;
  }
  if (typeof value === 'object') {
    const entries = Object.entries(value as Record<string, unknown>);
    if (entries.length === 0) return '{}\n';
    let out = '';
    for (const [k, v] of entries) {
      out += `${' '.repeat(indent)}${k}:${formatRhs(v, indent)}`;
    }
    return out;
  }
  return `${String(value)}\n`;
}

function dumpInline(value: unknown, indent: number): string {
  if (typeof value === 'object' && value !== null && !Array.isArray(value)) {
    const entries = Object.entries(value as Record<string, unknown>);
    if (entries.length === 0) return '{}\n';
    // First key follows the "- " inline, subsequent keys are indented.
    const [firstKey, firstVal] = entries[0]!;
    let out = `${firstKey}:${formatRhs(firstVal, indent)}`;
    for (const [k, v] of entries.slice(1)) {
      out += `${' '.repeat(indent)}${k}:${formatRhs(v, indent)}`;
    }
    return out;
  }
  return dumpYaml(value, indent);
}

function formatRhs(value: unknown, indent: number): string {
  if (value === null || value === undefined) return ' null\n';
  if (typeof value === 'boolean') return ` ${value}\n`;
  if (typeof value === 'number') return ` ${value}\n`;
  if (typeof value === 'string') return ` ${quoteIfNeeded(value)}\n`;
  if (Array.isArray(value)) {
    if (value.length === 0) return ' []\n';
    return `\n${dumpYaml(value, indent + 2)}`;
  }
  if (typeof value === 'object') {
    const entries = Object.entries(value as Record<string, unknown>);
    if (entries.length === 0) return ' {}\n';
    return `\n${dumpYaml(value, indent + 2)}`;
  }
  return ` ${String(value)}\n`;
}

function quoteIfNeeded(s: string): string {
  // Quote when the string looks like a YAML reserved form, contains
  // special characters, or could be misread as a number / bool.
  if (s === '') return '""';
  if (/^(true|false|null|~|-?\d+(\.\d+)?)$/.test(s)) return `"${s}"`;
  if (/[:\n#&*?[\]{}|>!%@`,"']/.test(s)) return JSON.stringify(s);
  if (/^\s|\s$/.test(s)) return JSON.stringify(s);
  return s;
}

function textEncode(s: string): Uint8Array {
  return new TextEncoder().encode(s);
}
