#!/usr/bin/env bun
// Render-throughput baseline + regression guard for `@akua-dev/sdk`.
//
// Runs `Akua.renderSource()` against a fixed minimal Package N times,
// reports p50 / p95 / p99 / max in milliseconds, and exits 1 if either
// the warm-call median exceeds `WARM_BUDGET_MS` or the cold-call total
// exceeds `COLD_BUDGET_MS`.
//
// Usage:
//   bun run packages/sdk/bench/render-source.bench.ts          # report only
//   bun run packages/sdk/bench/render-source.bench.ts --check  # enforce budget
//
// Why bun: the SDK ships compiled JS but the bench imports the TS
// source directly so changes don't require a rebuild between runs.
// Cross-runtime numbers (Node 22, Deno) follow the same shape — see
// `docs/sdk-runtime-compat.md` for the matrix.

import { performance } from 'node:perf_hooks';
import { Akua } from '../src/mod.ts';

// Minimal KCL source — `renderSource` is the hot path the SDK is
// optimized for. Anything more complex (helm.template, kustomize.build)
// is dominated by the engine call, not the SDK overhead.
const SOURCE = `
schema Input:
    appName: str
    replicas: int = 2

input: Input = option("input") or Input {}

resources = [{
    apiVersion: "v1"
    kind: "ConfigMap"
    metadata.name: input.appName
    data.replicas: str(input.replicas)
}]
`;

const ITERATIONS = 100;
// Budgets are deliberately loose — a 50% regression is the signal
// we want, not a 1% drift. Tighten when CI surfaces real drift.
const WARM_BUDGET_MS = 25; // p50 of warm calls
const COLD_BUDGET_MS = 500; // first call (worst case: cold cwasm load)

interface Stats {
	p50: number;
	p95: number;
	p99: number;
	max: number;
	mean: number;
}

function quantile(sorted: number[], q: number): number {
	const idx = Math.min(sorted.length - 1, Math.floor(sorted.length * q));
	return sorted[idx];
}

function summarize(samples: number[]): Stats {
	const sorted = [...samples].sort((a, b) => a - b);
	const sum = sorted.reduce((a, b) => a + b, 0);
	return {
		p50: quantile(sorted, 0.5),
		p95: quantile(sorted, 0.95),
		p99: quantile(sorted, 0.99),
		max: sorted[sorted.length - 1],
		mean: sum / sorted.length,
	};
}

function fmt(ms: number): string {
	return `${ms.toFixed(2)} ms`;
}

const akua = new Akua();

const samples: number[] = [];
for (let i = 0; i < ITERATIONS; i++) {
	const t0 = performance.now();
	await akua.renderSource({
		source: SOURCE,
		inputs: { appName: `bench-${i}` },
	});
	samples.push(performance.now() - t0);
}

const cold = samples[0];
const warm = summarize(samples.slice(1));

console.log(`render-source benchmark — ${ITERATIONS} iterations`);
console.log(`  cold (first call): ${fmt(cold)}`);
console.log(`  warm p50:          ${fmt(warm.p50)}`);
console.log(`  warm p95:          ${fmt(warm.p95)}`);
console.log(`  warm p99:          ${fmt(warm.p99)}`);
console.log(`  warm max:          ${fmt(warm.max)}`);
console.log(`  warm mean:         ${fmt(warm.mean)}`);

const enforce = process.argv.includes('--check');
if (enforce) {
	const failures: string[] = [];
	if (cold > COLD_BUDGET_MS) {
		failures.push(
			`cold call ${fmt(cold)} exceeds budget ${fmt(COLD_BUDGET_MS)}`,
		);
	}
	if (warm.p50 > WARM_BUDGET_MS) {
		failures.push(
			`warm p50 ${fmt(warm.p50)} exceeds budget ${fmt(WARM_BUDGET_MS)}`,
		);
	}
	if (failures.length > 0) {
		console.error('\nFAIL — perf regression:');
		for (const f of failures) console.error(`  ${f}`);
		process.exit(1);
	}
	console.log('\nOK — within budget');
}
