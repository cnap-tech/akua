import { execFile } from 'node:child_process';

import type { VersionOutput } from './types/VersionOutput.ts';
import type { WhoamiOutput } from './types/WhoamiOutput.ts';

import { classifyCliError } from './errors.ts';
import { type SchemaName, validateAs } from './validate.ts';

// The WASM bundle is CommonJS (wasm-pack `--target nodejs`). Lazy
// `await import` keeps it off the module graph until the first
// in-process render — the shell-out verbs don't need it.
type WasmBinding = {
	render: (packageFilename: string, source: string, inputsJson: string | null) => string;
	version: () => string;
};
let wasmPromise: Promise<WasmBinding> | undefined;
function loadWasm(): Promise<WasmBinding> {
	if (!wasmPromise) {
		wasmPromise = import('../wasm/nodejs/akua_wasm.js') as Promise<WasmBinding>;
	}
	return wasmPromise;
}

export * from './errors.ts';
export { AkuaContractError, standardSchemaFor, validateAs } from './validate.ts';
export type { SchemaName } from './validate.ts';
export type { VersionOutput, WhoamiOutput };
export type { ExitCode } from './types/ExitCode.ts';
export type { StructuredError, Level } from './types/StructuredError.ts';
export type { AgentContext } from './types/AgentContext.ts';
export type { AgentSource } from './types/AgentSource.ts';

// 64 MiB covers large `inspect` / `render` outputs. Verbs that stream
// multi-hundred-MiB artifacts should use a streaming API instead of
// buffering into a single string (not this class).
const DEFAULT_MAX_BUFFER = 64 * 1024 * 1024;

export interface AkuaOptions {
	/** Path to the `akua` binary. Defaults to `"akua"` (resolved via PATH). */
	binary?: string;
}

/**
 * Thin wrapper around the `akua` CLI. Each method shells out to a verb,
 * parses the `--json` output, and returns a value typed by the ts-rs
 * generated types. Failures throw the right `AkuaError` subclass based
 * on exit code + parsed StructuredError.
 */
export class Akua {
	readonly binary: string;

	constructor(opts: AkuaOptions = {}) {
		this.binary = opts.binary ?? 'akua';
	}

	version(): Promise<VersionOutput> {
		return this.call('version', 'VersionOutput');
	}

	whoami(): Promise<WhoamiOutput> {
		return this.call('whoami', 'WhoamiOutput');
	}

	/**
	 * Evaluate a Package.k source buffer against optional inputs and
	 * return the rendered top-level YAML. Runs entirely in-process via
	 * the bundled `akua-wasm` module — no `akua` binary required. KCL
	 * plugin callouts (`helm.template`, `kustomize.build`,
	 * `pkg.render`) are not yet available in the WASM bundle; Packages
	 * that use them surface a `__kcl_PanicInfo__` error via the
	 * backing KCL runtime. Use the CLI binary path (this class's
	 * other verbs) when plugin callouts are required.
	 *
	 * `packageFilename` is used for diagnostic rendering only — no
	 * filesystem is touched. `inputs` is optional; pass any
	 * JSON-serializable value to inject as KCL's `option("input")`.
	 */
	async renderSource(
		packageFilename: string,
		source: string,
		inputs?: unknown,
	): Promise<string> {
		const wasm = await loadWasm();
		const inputsJson = inputs === undefined ? null : JSON.stringify(inputs);
		return wasm.render(packageFilename, source, inputsJson);
	}

	private async call<T>(verb: string, schema: SchemaName): Promise<T> {
		const { stdout } = await this.exec([verb, '--json']);
		return validateAs<T>(schema, JSON.parse(stdout));
	}

	private exec(args: readonly string[]): Promise<{ stdout: string; stderr: string }> {
		return new Promise((resolve, reject) => {
			const child = execFile(
				this.binary,
				args,
				{ encoding: 'utf8', maxBuffer: DEFAULT_MAX_BUFFER },
				(err, stdout, stderr) => {
					if (!err) {
						resolve({ stdout, stderr });
						return;
					}
					// Both binary-not-found (ENOENT, exitCode == null) and non-zero
					// exits land here — `classifyCliError` picks the right subclass.
					reject(classifyCliError({ rawExitCode: child.exitCode, stderr, cause: err }));
				},
			);
		});
	}
}
