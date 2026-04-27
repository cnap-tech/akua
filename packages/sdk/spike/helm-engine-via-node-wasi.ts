// Spike #2 (#458 stage 2): host helm-engine.wasm via Node `node:wasi`
// directly — no worker, no plugin bridge. Tarballs a chart dir,
// invokes `helm_render`, parses the manifests envelope. Validates
// the engine's wasip1 ABI (_initialize + helm_malloc/free/render/
// result_len) is callable end-to-end from the JS host before we
// wire it into the worker's plugin bridge.
//
// Run:  node packages/sdk/spike/helm-engine-via-node-wasi.ts
//
// Engine ABI mirror — kept close to crates/engine-host-wasm/src/lib.rs
// so divergence is mechanical, not architectural.

import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { WASI } from 'node:wasi';
import { create as createTar } from 'tar';

const HERE = dirname(fileURLToPath(import.meta.url));
const HELM_ENGINE_WASM = resolve(
	HERE,
	'../../../crates/helm-engine-wasm/assets/helm-engine.wasm',
);
const CHART_DIR = resolve(HERE, '../../../examples/00-helm-hello/chart');

interface Release {
	name: string;
	namespace: string;
	revision: number;
	service: string;
}

interface HelmEngineSession {
	memory: WebAssembly.Memory;
	malloc: (size: number) => number;
	free: (ptr: number) => void;
	render: (inputPtr: number, inputLen: number) => number;
	resultLen: (ptr: number) => number;
}

async function loadHelmEngine(wasmPath: string): Promise<HelmEngineSession> {
	// `argv[0]` becomes the engine's process name visible to WASI;
	// matches the EngineSpec.name in engine-host-wasm.
	const wasi = new WASI({
		version: 'preview1',
		args: ['helm-engine'],
		env: {},
		preopens: {},
	});

	const wasm = readFileSync(wasmPath);
	const module_ = await WebAssembly.compile(wasm);

	const imports: WebAssembly.Imports = {
		wasi_snapshot_preview1: wasi.wasiImport,
	};

	const instance = await WebAssembly.instantiate(module_, imports);

	// Reactor module: _initialize runs Go's package-init chain
	// (klog setup, helm SDK inits). Must run exactly once before any
	// helm_* call — same constraint engine-host-wasm enforces via
	// wasmtime's per-thread Session.
	wasi.initialize(instance);

	const exports = instance.exports as Record<string, WebAssembly.ExportValue>;
	const requireExport = <T>(name: string): T => {
		const value = exports[name];
		if (value === undefined) {
			throw new Error(`helm-engine.wasm missing export \`${name}\``);
		}
		return value as T;
	};

	return {
		memory: requireExport<WebAssembly.Memory>('memory'),
		malloc: requireExport<(size: number) => number>('helm_malloc'),
		free: requireExport<(ptr: number) => void>('helm_free'),
		render: requireExport<(inputPtr: number, inputLen: number) => number>('helm_render'),
		resultLen: requireExport<(ptr: number) => number>('helm_result_len'),
	};
}

function copyIn(session: HelmEngineSession, bytes: Uint8Array): number {
	const ptr = session.malloc(bytes.length);
	new Uint8Array(session.memory.buffer, ptr, bytes.length).set(bytes);
	return ptr;
}

function copyOut(session: HelmEngineSession, ptr: number, len: number): Uint8Array {
	return new Uint8Array(session.memory.buffer, ptr, len).slice();
}

async function tarChartDir(chartDir: string, chartName: string): Promise<Uint8Array> {
	// Mirrors `crates/helm-engine-wasm/src/lib.rs::tar_chart_dir`: tar
	// entries are `<chartName>/<rel>` (engine looks for `<chartName>/
	// Chart.yaml`). cwd = parent dir, target = basename, then rename
	// the entry prefix to `chartName` so the tar layout is independent
	// of the on-disk dir name.
	const { basename } = await import('node:path');
	const onDisk = basename(chartDir);
	const chunks: Uint8Array[] = [];
	const stream = createTar(
		{
			gzip: true,
			cwd: dirname(chartDir),
			// Rewrite each entry path: strip the on-disk dir name, prefix
			// with the canonical chartName the engine expects. Three
			// cases cover the shapes node-tar emits: bare dir name,
			// dir name with trailing slash, and any nested path.
			onWriteEntry(entry) {
				const p = entry.path;
				if (p === onDisk || p === `${onDisk}/`) {
					entry.path = `${chartName}/`;
				} else if (p.startsWith(`${onDisk}/`)) {
					entry.path = `${chartName}/${p.slice(onDisk.length + 1)}`;
				}
			},
		},
		[onDisk],
	) as NodeJS.ReadableStream;
	for await (const chunk of stream) {
		chunks.push(typeof chunk === 'string' ? new TextEncoder().encode(chunk) : new Uint8Array(chunk));
	}
	const total = chunks.reduce((n, c) => n + c.length, 0);
	const out = new Uint8Array(total);
	let off = 0;
	for (const c of chunks) {
		out.set(c, off);
		off += c.length;
	}
	return out;
}

async function helmRender(
	session: HelmEngineSession,
	chartTarGz: Uint8Array,
	valuesYaml: string,
	release: Release,
): Promise<Record<string, string>> {
	const b64 = Buffer.from(chartTarGz).toString('base64');
	const request = {
		chart_tar_gz_b64: b64,
		values_yaml: valuesYaml,
		release,
	};
	const input = new TextEncoder().encode(JSON.stringify(request));

	const inputPtr = copyIn(session, input);
	const resultPtr = session.render(inputPtr, input.length);
	const len = session.resultLen(resultPtr);
	const outBytes = copyOut(session, resultPtr, len);
	// Best-effort free; engine reuses freed pointers on next alloc,
	// so a missed free here costs only fragmentation.
	session.free(inputPtr);
	session.free(resultPtr);

	const response = JSON.parse(new TextDecoder().decode(outBytes)) as {
		manifests?: Record<string, string>;
		error?: string;
	};
	if (response.error) {
		throw new Error(`helm engine: ${response.error}`);
	}
	return response.manifests ?? {};
}

// ---------------------------------------------------------------------------

const t0 = performance.now();
const session = await loadHelmEngine(HELM_ENGINE_WASM);
const t1 = performance.now();
console.log(`[spike2] loadHelmEngine: ${(t1 - t0).toFixed(1)} ms`);

// Tar the chart dir. examples/00-helm-hello/chart contains a
// minimal Helm chart that renders one Deployment.
const chartTar = await tarChartDir(CHART_DIR, 'chart');
const t2 = performance.now();
console.log(`[spike2] tar chart: ${(t2 - t1).toFixed(1)} ms (${chartTar.length} bytes)`);

const release: Release = {
	name: 'spike',
	namespace: 'default',
	revision: 1,
	service: 'Helm',
};

const manifests = await helmRender(session, chartTar, '', release);
const t3 = performance.now();
console.log(`[spike2] helm render: ${(t3 - t2).toFixed(1)} ms`);

console.log('[spike2] manifests:');
for (const [path, yaml] of Object.entries(manifests)) {
	console.log(`--- ${path} ---`);
	console.log(yaml.slice(0, 200) + (yaml.length > 200 ? '...' : ''));
}

console.log('[spike2] ✅ helm-engine.wasm hosted via node:wasi');
