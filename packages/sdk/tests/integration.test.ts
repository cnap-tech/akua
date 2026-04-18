/**
 * Real-world integration tests — hit public registries to catch
 * behaviour drift before users do. Skipped by default; enable with
 * `AKUA_E2E=1 bun run test`.
 *
 * Covers:
 *   - OCI pull from GHCR (podinfo, public, anonymous bearer dance).
 *   - HTTP Helm pull from Jetstack (cert-manager, standard index.yaml).
 *   - HTTP Helm index caching (two pulls → one index.yaml fetch).
 */

import { describe, it, expect, beforeAll } from 'vitest';

import { init, pullChart, inspectChartBytes } from '../src/index.node.ts';
import { clearIndexCache } from '../src/helm-http.ts';

const RUN = process.env.AKUA_E2E === '1';
const describeE2E = RUN ? describe : describe.skip;

beforeAll(async () => {
  if (!RUN) return;
  await init();
});

describeE2E('E2E — OCI (ghcr.io)', () => {
  it('pulls podinfo chart and inspects it', async () => {
    const bytes = await pullChart('oci://ghcr.io/stefanprodan/charts/podinfo:6.7.1');
    expect(bytes.byteLength).toBeGreaterThan(0);
    const info = await inspectChartBytes(bytes);
    const chartYaml = info.chartYaml as { name: string; version: string };
    expect(chartYaml.name).toBe('podinfo');
    expect(chartYaml.version).toBe('6.7.1');
  }, 30_000);
});

describeE2E('E2E — HTTP Helm (charts.jetstack.io)', () => {
  it('pulls cert-manager chart via index.yaml lookup', async () => {
    clearIndexCache();
    const bytes = await pullChart('https://charts.jetstack.io/cert-manager:v1.16.1');
    expect(bytes.byteLength).toBeGreaterThan(0);
    const info = await inspectChartBytes(bytes);
    const chartYaml = info.chartYaml as { name: string; version: string };
    expect(chartYaml.name).toBe('cert-manager');
    expect(chartYaml.version).toBe('v1.16.1');
  }, 60_000);

  it('index.yaml is cached across pulls', async () => {
    clearIndexCache();
    const realFetch = globalThis.fetch;
    const calls: string[] = [];
    globalThis.fetch = (async (url: string | URL, opts?: RequestInit) => {
      calls.push(url.toString());
      return realFetch(url, opts);
    }) as unknown as typeof fetch;
    try {
      await pullChart('https://charts.jetstack.io/cert-manager:v1.16.1');
      await pullChart('https://charts.jetstack.io/cert-manager:v1.16.0');
      const indexCalls = calls.filter((u) => u.endsWith('/index.yaml'));
      expect(indexCalls.length).toBe(1);
    } finally {
      globalThis.fetch = realFetch;
    }
  }, 60_000);
});
