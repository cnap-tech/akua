// Spike (#458): can Node 22+ `node:wasi` host the existing
// `akua-render-worker.wasm` (wasm32-wasip1, built for the CLI's
// wasmtime path) and execute a pure-KCL render end-to-end?
//
// Decision gate for the SDK feature-parity track (#458–#467). If this
// works, the engine bridge (#459) is incremental — replicate the
// wasmtime plugin-bridge JS-side using helm-engine.wasm /
// kustomize-engine.wasm as additional Node WASI instances.
//
// Run:  bun run packages/sdk/spike/render-worker-via-node-wasi.ts

import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { WASI } from 'node:wasi';

const HERE = dirname(fileURLToPath(import.meta.url));
const WORKER_WASM = resolve(
	HERE,
	'../../../target/wasm32-wasip1/release/akua-render-worker.wasm',
);

// Pure-KCL Package: no `helm.template`, no `kustomize.build`, no
// `charts.*` imports. The minimum viable input that exercises KCL
// eval + the worker's stdin/stdout protocol without needing the
// plugin bridge. If this round-trips, the WASI host is sound.
const PURE_KCL_SOURCE = `
schema Input:
    replicas: int = 2

input: Input = option("input") or Input {}

resources = [{
    apiVersion: "v1"
    kind: "ConfigMap"
    metadata.name: "spike"
    data.count: str(input.replicas)
}]
`;

const REQUEST = JSON.stringify({
	kind: 'render',
	package_filename: 'package.k',
	source: PURE_KCL_SOURCE,
	inputs: { replicas: 5 },
});

// Worker reads a JSON request from stdin, writes one JSON response to
// stdout. We feed the request via a memory-backed virtual FD using
// node:wasi's experimental `stdin` option — but that only takes an
// fd, not a buffer. The simplest path: write request to a real temp
// file and preopen its containing dir as the worker's input.
//
// Actually node:wasi accepts `stdin: number` (file descriptor). We
// open a pipe pair, write the request to the write end, hand the
// read end to WASI. Same shape the CLI's wasmtime host uses.
import { mkdtempSync, openSync, writeFileSync, readFileSync as readFile } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const tmp = mkdtempSync(join(tmpdir(), 'akua-spike-'));
const stdinPath = join(tmp, 'stdin');
const stdoutPath = join(tmp, 'stdout');
const stderrPath = join(tmp, 'stderr');
writeFileSync(stdinPath, REQUEST);
writeFileSync(stdoutPath, '');
writeFileSync(stderrPath, '');

const stdinFd = openSync(stdinPath, 'r');
const stdoutFd = openSync(stdoutPath, 'w');
const stderrFd = openSync(stderrPath, 'w');

const wasi = new WASI({
	version: 'preview1',
	args: ['akua-render-worker'],
	env: {},
	stdin: stdinFd,
	stdout: stdoutFd,
	stderr: stderrFd,
	preopens: {},
});

const t0 = performance.now();

const wasm = readFileSync(WORKER_WASM);
const module_ = await WebAssembly.compile(wasm);

// Bun's `node:wasi` exposes imports via `wasiImport` (object), Node's
// also exposes `getImportObject()` (function). The property works on
// both runtimes.
const imports: WebAssembly.Imports = {
	wasi_snapshot_preview1: wasi.wasiImport,
	// The worker imports `env.kcl_plugin_invoke_json_wasm` from the
	// wasmtime host. Pure-KCL doesn't invoke it; stub with a guard so
	// instantiate succeeds and surface unexpected calls if any.
	env: {
		kcl_plugin_invoke_json_wasm: (
			methodPtr: number,
			argsPtr: number,
			kwargsPtr: number,
		): number => {
			console.error(
				`[spike] plugin bridge fired unexpectedly (method_ptr=${methodPtr}, args_ptr=${argsPtr}, kwargs_ptr=${kwargsPtr}) — pure-KCL package should not call plugins`,
			);
			return 0;
		},
	},
};

const instance = await WebAssembly.instantiate(module_, imports);

const t1 = performance.now();
console.log(`[spike] instantiate: ${(t1 - t0).toFixed(1)} ms`);

// Node's WASI raises a special error on `proc_exit` — we have to
// inspect it instead of treating any throw as failure. A clean
// `proc_exit(0)` from the worker (which it does on every successful
// render) shows up as `err.code === 'ERR_WASI_EXIT_CODE'` with
// `err.exitCode === 0`. Treat that as success.
let exitCode = 0;
try {
	wasi.start(instance);
} catch (err) {
	const e = err as { code?: string; exitCode?: number };
	if (e.code === 'ERR_WASI_EXIT_CODE' && typeof e.exitCode === 'number') {
		exitCode = e.exitCode;
	} else {
		exitCode = 1;
		console.error('[spike] wasi.start threw:', err);
	}
}

const t2 = performance.now();
console.log(`[spike] wasi.start: ${(t2 - t1).toFixed(1)} ms`);

const stdout = readFile(stdoutPath, 'utf8');
const stderr = readFile(stderrPath, 'utf8');

console.log('[spike] exit code:', exitCode);
console.log('[spike] stderr:', stderr || '(empty)');
console.log('[spike] stdout:');
console.log(stdout);

if (exitCode === 0 && stdout) {
	const response = JSON.parse(stdout);
	console.log('[spike] parsed response:', JSON.stringify(response, null, 2));
	if (response.kind === 'render' && response.status === 'ok') {
		console.log(
			'[spike] ✅ DECISION GATE PASSED — render-worker.wasm runs under node:wasi',
		);
		console.log('[spike] yaml output:');
		console.log(response.yaml);
		process.exit(0);
	}
}
console.log(
	'[spike] ❌ DECISION GATE FAILED — see stderr / response above',
);
process.exit(1);
