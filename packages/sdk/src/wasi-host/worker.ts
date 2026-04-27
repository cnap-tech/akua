// In-process render host — runs `akua-render-worker.wasm` under
// `node:wasi` and bridges its `kcl_plugin_invoke_json_wasm` import
// to JS-side helm + kustomize engine drivers. Mirrors the host
// contract `crates/akua-cli/src/render_worker.rs` provides via
// wasmtime, in TypeScript over Node WASI.
//
// Lifecycle: `RenderHost.create()` once per SDK process — compiles
// the worker module, lazily loads each engine on first use. Per
// render: build a fresh `WASI` (clean stdin/stdout fds) + fresh
// `WebAssembly.Instance`, run `wasi.start()`. The compiled module
// is reused; the plugin-bridge state is per-render closure-bound.

import { copyFileSync, mkdtempSync, openSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { WASI } from 'node:wasi';

import { Engine } from './engine.ts';
import { makeHelmTemplateHandler } from './helm.ts';
import { makeKustomizeBuildHandler } from './kustomize.ts';
import { readCString, readCStringOr, writeCStringTo } from './c-string.ts';

const PANIC_INFO_KEY = '__kcl_PanicInfo__';

type PluginHandler = (argsJson: string, kwargsJson: string) => unknown;

interface ResolveOptions {
	/**
	 * Workspace directory containing the Package source on disk —
	 * used by `helm.template` / `kustomize.build` to resolve relative
	 * chart / overlay paths. Required when the Package invokes either
	 * plugin; pure-KCL Packages can omit it.
	 */
	packageDir?: string;
}

export interface RenderRequest extends ResolveOptions {
	/** Diagnostic filename, used by KCL for error spans. */
	packageFilename: string;
	source: string;
	/** Optional inputs JSON-injected as KCL's `option("input")`. */
	inputs?: unknown;
}

export interface RenderResult {
	yaml: string;
}

interface RenderWorkerArtifacts {
	workerWasmPath: string;
	helmEngineWasmPath: string;
	kustomizeEngineWasmPath: string;
	/**
	 * Source dir holding akua's KCL stdlib (`ctx.k`, `helm.k`,
	 * `kustomize.k`, `pkg.k`). Materialized at `RenderHost.create()`
	 * into a tempdir with a generated `kcl.mod` and preopened at
	 * `/akua-stdlib` inside the worker. Mirrors what the CLI's
	 * `akua_core::stdlib::stdlib_root` produces at runtime.
	 */
	stdlibSourcePath: string;
	/** Materialized stdlib tempdir — set by `RenderHost.create`. */
	stdlibPath: string;
}

/**
 * In-process render host. Caches the compiled worker module across
 * renders; engines load lazily on first use of their plugin.
 */
export class RenderHost {
	private readonly module: WebAssembly.Module;
	private readonly artifacts: RenderWorkerArtifacts;
	private helm?: Promise<Engine>;
	private kustomize?: Promise<Engine>;

	private constructor(module: WebAssembly.Module, artifacts: RenderWorkerArtifacts) {
		this.module = module;
		this.artifacts = artifacts;
	}

	static async create(artifacts?: Partial<RenderWorkerArtifacts>): Promise<RenderHost> {
		const resolved = resolveArtifacts(artifacts);
		const stdlibPath = materializeStdlib(resolved.stdlibSourcePath);
		const wasm = readFileSync(resolved.workerWasmPath);
		const module_ = await WebAssembly.compile(wasm);
		return new RenderHost(module_, { ...resolved, stdlibPath });
	}

	private async helmEngine(): Promise<Engine> {
		if (!this.helm) {
			this.helm = Engine.load({
				wasmPath: this.artifacts.helmEngineWasmPath,
				prefix: 'helm',
				entry: 'helm_render',
				name: 'helm-engine',
			});
		}
		return this.helm;
	}

	private async kustomizeEngine(): Promise<Engine> {
		if (!this.kustomize) {
			this.kustomize = Engine.load({
				wasmPath: this.artifacts.kustomizeEngineWasmPath,
				prefix: 'kustomize',
				entry: 'kustomize_build',
				name: 'kustomize-engine',
			});
		}
		return this.kustomize;
	}

	/**
	 * Render `request`. Pre-resolves the engine handlers eagerly when
	 * the source mentions either plugin (cheap string-contains probe)
	 * so the sync plugin bridge later can dispatch without an `await`.
	 *
	 * **Hang caveat:** `wasi.start()` is synchronous and Node WASI
	 * doesn't expose epoch / wall-clock interrupts. A Package with an
	 * infinite loop in KCL eval blocks the host thread until the
	 * process exits. Treat untrusted Packages with care; trusted
	 * authoring + CI workflows don't hit this. Tracked under #465.
	 */
	async render(request: RenderRequest): Promise<RenderResult> {
		const probe = request.source;
		const wantHelm = mentions(probe, 'helm.');
		const wantKustomize = mentions(probe, 'kustomize.');

		// Engine cold-load is ~140ms each. For Packages that use both,
		// pre-load in parallel — saves one whole engine-init wall-time
		// on first render. Subsequent renders hit the cached promises.
		const [helmEngine, kustomizeEngine] = await Promise.all([
			wantHelm ? this.helmEngine() : Promise.resolve(undefined),
			wantKustomize ? this.kustomizeEngine() : Promise.resolve(undefined),
		]);

		const handlers = new Map<string, PluginHandler>();
		if (helmEngine) {
			handlers.set(
				'helm.template',
				makeHelmTemplateHandler({
					engine: helmEngine,
					packageDir: requirePackageDir(request, 'helm.template'),
				}),
			);
		}
		if (kustomizeEngine) {
			handlers.set(
				'kustomize.build',
				makeKustomizeBuildHandler({
					engine: kustomizeEngine,
					packageDir: requirePackageDir(request, 'kustomize.build'),
				}),
			);
		}

		const requestJson = JSON.stringify({
			kind: 'render',
			package_filename: request.packageFilename,
			source: request.source,
			inputs: request.inputs ?? null,
		});

		const { stdout, stderr, exitCode } = runWorkerOnce({
			module: this.module,
			requestJson,
			handlers,
			stdlibPath: this.artifacts.stdlibPath,
		});

		if (exitCode !== 0) {
			throw new Error(
				`render-worker exited with code ${exitCode}\nstderr: ${stderr || '(empty)'}`,
			);
		}
		const response = parseWorkerResponse(stdout);
		if (response.kind !== 'render') {
			throw new Error(`unexpected worker response kind: ${response.kind}`);
		}
		if (response.status !== 'ok') {
			throw new Error(response.message || 'render failed (no diagnostic)');
		}
		return { yaml: response.yaml };
	}
}

interface WorkerInvocation {
	module: WebAssembly.Module;
	requestJson: string;
	handlers: Map<string, PluginHandler>;
	stdlibPath: string;
}

interface WorkerInvocationResult {
	stdout: string;
	stderr: string;
	exitCode: number;
}

interface WorkerRenderResponse {
	kind: 'render';
	status: 'ok' | 'fail';
	yaml: string;
	message?: string;
}

interface WorkerPingResponse {
	kind: 'ping';
}

type WorkerResponse = WorkerRenderResponse | WorkerPingResponse;

function runWorkerOnce(inv: WorkerInvocation): WorkerInvocationResult {
	// Build a fresh WASI instance per render so stdin/stdout fds are
	// clean. Tempfile-backed fds are sub-ms on modern storage; the
	// alternative (in-memory pipes via Atomics) is significantly more
	// complex and not justified at this throughput.
	const tmp = mkdtempSync(join(tmpdir(), 'akua-sdk-render-'));
	const stdinPath = join(tmp, 'stdin');
	const stdoutPath = join(tmp, 'stdout');
	const stderrPath = join(tmp, 'stderr');
	writeFileSync(stdinPath, inv.requestJson);
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
		preopens: {
			// `import akua.{helm,kustomize,pkg,ctx}` resolves against
			// this mount — the worker registers an ExternalPkg `akua`
			// → `/akua-stdlib`. Mirrors the CLI's wasmtime preopen.
			'/akua-stdlib': inv.stdlibPath,
		},
	});

	// Bridge state — captured by the plugin import function. We need
	// to know the worker's memory + alloc export, but those only
	// exist after `instantiate`. Using a placeholder + lazy lookup
	// inside the import body keeps the import object construction
	// independent of the instance.
	let memory: WebAssembly.Memory | undefined;
	let alloc: ((size: number) => number) | undefined;

	const imports: WebAssembly.Imports = {
		wasi_snapshot_preview1: wasi.wasiImport,
		env: {
			kcl_plugin_invoke_json_wasm: (
				methodPtr: number,
				argsPtr: number,
				kwargsPtr: number,
			): number => {
				if (!memory || !alloc) {
					// Should never fire — the import is only called from
					// guest code, which runs after instantiate.
					throw new Error('plugin bridge fired before worker instance was bound');
				}
				return dispatchPlugin(memory, alloc, methodPtr, argsPtr, kwargsPtr, inv.handlers);
			},
		},
	};

	let exitCode = 0;
	let stdout = '';
	let stderr = '';
	try {
		const instance = new WebAssembly.Instance(inv.module, imports);
		memory = instance.exports.memory as WebAssembly.Memory;
		alloc = instance.exports.akua_bridge_alloc as (size: number) => number;
		wasi.start(instance);
	} catch (err) {
		const e = err as { code?: string; exitCode?: number };
		if (e.code === 'ERR_WASI_EXIT_CODE' && typeof e.exitCode === 'number') {
			exitCode = e.exitCode;
		} else {
			exitCode = 1;
			stderr += `\nwasi.start threw: ${err instanceof Error ? err.message : String(err)}`;
		}
	} finally {
		stdout = readFileSync(stdoutPath, 'utf8');
		stderr += readFileSync(stderrPath, 'utf8');
		// Best-effort cleanup; tempdir is in $TMPDIR so OS reaping
		// covers a missed call.
		try {
			rmSync(tmp, { recursive: true, force: true });
		} catch {
			/* drop */
		}
	}
	return { stdout, stderr, exitCode };
}

function dispatchPlugin(
	memory: WebAssembly.Memory,
	alloc: (size: number) => number,
	methodPtr: number,
	argsPtr: number,
	kwargsPtr: number,
	handlers: Map<string, PluginHandler>,
): number {
	const responseJson = invokeBridge(memory, methodPtr, argsPtr, kwargsPtr, handlers);
	return writeCStringTo(memory, alloc, responseJson);
}

function invokeBridge(
	memory: WebAssembly.Memory,
	methodPtr: number,
	argsPtr: number,
	kwargsPtr: number,
	handlers: Map<string, PluginHandler>,
): string {
	const rawMethod = readCString(memory, methodPtr);
	if (rawMethod === null) {
		return panicEnvelope('plugin dispatcher received null method name');
	}
	// KCL prefixes plugin names with `kcl_plugin.` — strip so handlers
	// register as their bare names (matches Rust host behavior).
	const method = rawMethod.startsWith('kcl_plugin.') ? rawMethod.slice('kcl_plugin.'.length) : rawMethod;

	const argsJson = readCStringOr(memory, argsPtr, '[]');
	const kwargsJson = readCStringOr(memory, kwargsPtr, '{}');

	const handler = handlers.get(method);
	if (!handler) {
		return panicEnvelope(`no plugin registered under \`${method}\``);
	}

	try {
		const value = handler(argsJson, kwargsJson);
		return JSON.stringify(value);
	} catch (err) {
		const msg = err instanceof Error ? err.message : String(err);
		return panicEnvelope(msg);
	}
}

function panicEnvelope(message: string): string {
	return JSON.stringify({ [PANIC_INFO_KEY]: message });
}

function parseWorkerResponse(stdout: string): WorkerResponse {
	try {
		return JSON.parse(stdout) as WorkerResponse;
	} catch (err) {
		throw new Error(
			`render-worker emitted non-JSON response: ${err instanceof Error ? err.message : err}\nstdout: ${stdout}`,
		);
	}
}

function mentions(source: string, needle: string): boolean {
	return source.includes(needle);
}

function requirePackageDir(req: RenderRequest, plugin: string): string {
	if (!req.packageDir) {
		throw new Error(
			`${plugin}: source invokes the plugin but no \`packageDir\` was provided — pass \`Akua.render({package: ...})\` (path mode) or set \`packageDir\` explicitly`,
		);
	}
	return req.packageDir;
}

function resolveArtifacts(overrides?: Partial<RenderWorkerArtifacts>): RenderWorkerArtifacts {
	const here = dirname(fileURLToPath(import.meta.url));
	const repoRoot = resolve(here, '../../../..');
	return {
		workerWasmPath:
			overrides?.workerWasmPath ??
			resolve(repoRoot, 'target/wasm32-wasip1/release/akua-render-worker.wasm'),
		helmEngineWasmPath:
			overrides?.helmEngineWasmPath ??
			resolve(repoRoot, 'crates/helm-engine-wasm/assets/helm-engine.wasm'),
		kustomizeEngineWasmPath:
			overrides?.kustomizeEngineWasmPath ??
			resolve(repoRoot, 'crates/kustomize-engine-wasm/assets/kustomize-engine.wasm'),
		stdlibSourcePath:
			overrides?.stdlibSourcePath ?? resolve(repoRoot, 'crates/akua-core/stdlib/akua'),
		stdlibPath: overrides?.stdlibPath ?? '', // populated by `materializeStdlib`
	};
}

/**
 * Write akua's KCL stdlib into a tempdir matching the on-disk shape
 * the worker expects: a flat dir with `kcl.mod` + `{ctx,helm,
 * kustomize,pkg}.k`. Mirrors `akua_core::stdlib::stdlib_root` —
 * which materializes embedded `include_str!` content into a
 * tempdir at first call. Cached for the host's lifetime.
 */
const KCL_MOD_AKUA = '[package]\nname = "akua"\nedition = "0.0.1"\nversion = "0.0.1"\n';
function materializeStdlib(sourceDir: string): string {
	const dir = mkdtempSync(join(tmpdir(), 'akua-sdk-stdlib-'));
	for (const name of ['ctx.k', 'helm.k', 'kustomize.k', 'pkg.k']) {
		copyFileSync(join(sourceDir, name), join(dir, name));
	}
	writeFileSync(join(dir, 'kcl.mod'), KCL_MOD_AKUA);
	registerCleanup(dir);
	return dir;
}

/**
 * Register a tempdir for cleanup on process exit. Bounded to one
 * `process.on('exit', ...)` registration per process; subsequent
 * calls just append to the list. Worth the ~free overhead in
 * daemon contexts (long-running API servers, Temporal workers)
 * where leaking a stdlib dir per host instance would accumulate.
 */
const cleanupDirs: string[] = [];
let cleanupRegistered = false;
function registerCleanup(dir: string): void {
	cleanupDirs.push(dir);
	if (cleanupRegistered) return;
	cleanupRegistered = true;
	process.on('exit', () => {
		for (const d of cleanupDirs) {
			try {
				rmSync(d, { recursive: true, force: true });
			} catch {
				/* drop — process is exiting anyway */
			}
		}
	});
}
