import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFile, readdir, stat, writeFile } from 'node:fs/promises';
import { posix as path } from 'node:path';

import type { CheckOutput } from './types/CheckOutput.ts';
import type { CheckResult } from './types/CheckResult.ts';
import type { DirDiff } from './types/DirDiff.ts';
import type { FileChange } from './types/FileChange.ts';
import type { FmtFile } from './types/FmtFile.ts';
import type { FmtOutput } from './types/FmtOutput.ts';
import type { InspectOutput } from './types/InspectOutput.ts';
import type { LintIssue } from './types/LintIssue.ts';
import type { LintOutput } from './types/LintOutput.ts';
import type { OptionInfo } from './types/OptionInfo.ts';
import type { RenderSummary } from './types/RenderSummary.ts';
import type { TreeOutput } from './types/TreeOutput.ts';
import type { VerifyOutput } from './types/VerifyOutput.ts';
import type { VersionOutput } from './types/VersionOutput.ts';
import type { WhoamiOutput } from './types/WhoamiOutput.ts';

import { classifyCliError } from './errors.ts';
import { type SchemaName, validateAs } from './validate.ts';

// Lazy-load the WASM bundle (~7.6 MB) so the SDK's shell-out verbs
// (`version`, `whoami`, `render`, `verify`) don't pay the parse
// cost when a consumer never touches an in-process verb. Static
// imports above cover all other dependencies.
type WasmBinding = {
	render: (packageFilename: string, source: string, inputsJson: string | null) => string;
	version: () => string;
	lint: (filename: string, source: string) => string;
	fmt: (filename: string, source: string, checkMode: boolean) => string;
	inspect_package: (filename: string, source: string) => string;
	check: (
		manifest: string | null,
		lock: string | null,
		packageFilename: string | null,
		packageSource: string | null,
	) => string;
	tree: (manifest: string, lock: string | null) => string;
	diff: (beforeJson: string, afterJson: string) => string;
};
let wasmPromise: Promise<WasmBinding> | undefined;
function loadWasm(): Promise<WasmBinding> {
	if (!wasmPromise) {
		wasmPromise = import('../wasm/nodejs/akua_wasm.js') as Promise<WasmBinding>;
	}
	return wasmPromise;
}

async function readOptional(p: string): Promise<string | undefined> {
	try {
		return await readFile(p, 'utf8');
	} catch (err: unknown) {
		const code = (err as { code?: string } | null | undefined)?.code;
		if (code === 'ENOENT') return undefined;
		throw err;
	}
}

/**
 * Walk `root` recursively and return `{ relPath: "sha256-hex" }`
 * for every regular file. Used by `Akua.diff` to hand the WASM
 * bundle two comparable manifests. Siblings are hashed in
 * parallel; the outer tree walk stays recursive so deep
 * hierarchies don't blow the task-queue budget.
 */
async function hashTree(root: string): Promise<Record<string, string>> {
	const rootStat = await stat(root);
	if (!rootStat.isDirectory()) {
		throw new Error(`diff: ${root} is not a directory`);
	}
	const out: Record<string, string> = {};
	async function walk(dir: string, rel: string): Promise<void> {
		const entries = await readdir(dir, { withFileTypes: true });
		await Promise.all(
			entries.map(async (entry) => {
				const absPath = path.join(dir, entry.name);
				const relPath = rel ? path.join(rel, entry.name) : entry.name;
				if (entry.isDirectory()) {
					await walk(absPath, relPath);
				} else if (entry.isFile()) {
					const bytes = await readFile(absPath);
					out[relPath] = createHash('sha256').update(bytes).digest('hex');
				}
				// Non-regular files (symlinks, sockets) are skipped —
				// mirrors `akua_core::dir_diff::diff`.
			}),
		);
	}
	await walk(root, '');
	return out;
}

export * from './errors.ts';
export { AkuaContractError, standardSchemaFor, validateAs } from './validate.ts';
export type { SchemaName } from './validate.ts';
export type {
	CheckOutput,
	CheckResult,
	DirDiff,
	FileChange,
	FmtFile,
	FmtOutput,
	InspectOutput,
	LintIssue,
	LintOutput,
	OptionInfo,
	RenderSummary,
	TreeOutput,
	VerifyOutput,
	VersionOutput,
	WhoamiOutput,
};
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

export interface InspectOptions {
	/** Path to the `package.k` file. Default: `./package.k`. Mutually exclusive with `tarball`. */
	package?: string;
	/** Path to a `.tar.gz` Package artifact. Mutually exclusive with `package`. */
	tarball?: string;
}

export interface TreeOptions {
	/** Workspace root (dir containing `akua.toml`). Default: `.`. */
	workspace?: string;
}

export interface VerifyOptions {
	/** Workspace root. Default: `.`. */
	workspace?: string;
	/** Path to a `.tar.gz` Package artifact for offline verify. */
	tarball?: string;
	/** Override the cosign public key path from `akua.toml [signing]`. */
	publicKey?: string;
}

export interface CheckOptions {
	/** Workspace root (dir containing `akua.toml` + `akua.lock`). Default: `.`. */
	workspace?: string;
	/** Path to the `package.k` file. Default: `./package.k`. */
	package?: string;
}

export interface LintOptions {
	/** Path to the `package.k` file. Default: `./package.k`. */
	package?: string;
}

export interface FmtOptions {
	/** Path to the `package.k` file. Default: `./package.k`. */
	package?: string;
	/** Fail with user-error exit if any file needs reformatting. */
	check?: boolean;
	/** Print formatted output to stdout instead of writing in place. */
	stdout?: boolean;
}

export interface RenderOptions {
	/** Path to the `package.k` file. Default: `./package.k`. */
	package?: string;
	/** Inputs file (JSON or YAML). */
	inputs?: string;
	/** Root directory where rendered YAML files land. Default: `./deploy`. */
	out?: string;
	dryRun?: boolean;
	/** Reject raw-string plugin paths — every chart must come from a typed `charts.*` import. */
	strict?: boolean;
	/** Forbid network access during resolve; OCI deps must cache-hit. */
	offline?: boolean;
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

	/**
	 * Render a Package on disk through the `akua` CLI, returning the
	 * typed summary (output path, manifest count, sha256 digest, file
	 * list). Shell-out transport — the CLI holds the filesystem state,
	 * dep resolution, and the embedded helm/kustomize engines.
	 * `renderSource` is the in-process counterpart for pure-KCL
	 * Packages that don't need any of that.
	 *
	 * Every field of `opts` maps to a `--` flag of `akua render`; see
	 * [`docs/cli.md#akua-render`](../../../docs/cli.md#akua-render).
	 */
	/**
	 * Fast syntax / type / dep check over the workspace. Runs
	 * entirely in-process via the bundled `akua-wasm` module — no
	 * `akua` binary required.
	 */
	async check(opts: CheckOptions = {}): Promise<CheckOutput> {
		const ws = opts.workspace ?? '.';
		const manifestPath = path.join(ws, 'akua.toml');
		const lockPath = path.join(ws, 'akua.lock');
		const pkgPath = opts.package ?? path.join(ws, 'package.k');

		const [manifest, lock, pkgSource] = await Promise.all([
			readOptional(manifestPath),
			readOptional(lockPath),
			readOptional(pkgPath),
		]);

		const wasm = await loadWasm();
		const coreOutput = JSON.parse(
			wasm.check(manifest ?? null, lock ?? null, pkgPath, pkgSource ?? null),
		) as CheckOutput;

		// CLI-parity: required-file failures the pure-compute core
		// doesn't gate on its own. Mirrors the CLI envelope so
		// CLI-JSON and SDK-JSON agree on scratch workspaces.
		const checks: CheckResult[] = [];
		if (manifest === undefined) {
			checks.push({ name: 'manifest', ok: false, error: `${manifestPath} not found`, issues: [] });
		}
		checks.push(...coreOutput.checks);
		if (pkgSource === undefined) {
			checks.push({ name: 'package', ok: false, error: `${pkgPath} not found`, issues: [] });
		}
		const status = checks.every((c) => c.ok) ? 'ok' : 'fail';
		return validateAs<CheckOutput>('CheckOutput', { status, checks });
	}

	/**
	 * Run the KCL linter against the Package. In-process via WASM —
	 * no binary required.
	 */
	async lint(opts: LintOptions = {}): Promise<LintOutput> {
		const pkg = opts.package ?? './package.k';
		const source = await readFile(pkg, 'utf8');
		const wasm = await loadWasm();
		return validateAs<LintOutput>('LintOutput', JSON.parse(wasm.lint(pkg, source)));
	}

	/**
	 * Format KCL sources. In-process via WASM — no binary required.
	 * With `check=true`, reports which files would change without
	 * touching disk. Without `check`, the formatted text is written
	 * back to the file (mirroring `akua fmt`'s in-place behavior).
	 */
	async fmt(opts: FmtOptions = {}): Promise<FmtOutput> {
		const pkg = opts.package ?? './package.k';
		const source = await readFile(pkg, 'utf8');
		const wasm = await loadWasm();
		const raw = JSON.parse(wasm.fmt(pkg, source, opts.check ?? false)) as {
			files: FmtFile[];
			formatted: string;
		};
		const shouldEmit = !opts.check && (raw.files[0]?.changed ?? false);
		if (shouldEmit && opts.stdout) {
			process.stdout.write(raw.formatted);
		} else if (shouldEmit) {
			await writeFile(pkg, raw.formatted, 'utf8');
		}
		return validateAs<FmtOutput>('FmtOutput', { files: raw.files });
	}

	/**
	 * Introspect a Package or a packed tarball — surface the option
	 * set, etc. Package mode runs in-process via WASM; tarball mode
	 * is not yet available in the WASM bundle (deferred to v0.2.0
	 * along with engine bundling) and currently throws.
	 */
	async inspect(opts: InspectOptions = {}): Promise<InspectOutput> {
		if (opts.tarball) {
			throw new Error(
				'Akua.inspect({ tarball }) is not yet available in the WASM bundle — use the akua CLI for tarball introspection.',
			);
		}
		const pkg = opts.package ?? './package.k';
		const source = await readFile(pkg, 'utf8');
		const wasm = await loadWasm();
		return validateAs<InspectOutput>(
			'InspectOutput',
			JSON.parse(wasm.inspect_package(pkg, source)),
		);
	}

	/**
	 * Print the workspace's declared deps + lockfile entries.
	 * In-process via WASM — no binary required.
	 */
	async tree(opts: TreeOptions = {}): Promise<TreeOutput> {
		const ws = opts.workspace ?? '.';
		const [manifest, lock] = await Promise.all([
			readFile(path.join(ws, 'akua.toml'), 'utf8'),
			readOptional(path.join(ws, 'akua.lock')),
		]);
		const wasm = await loadWasm();
		return validateAs<TreeOutput>('TreeOutput', JSON.parse(wasm.tree(manifest, lock ?? null)));
	}

	/**
	 * Verify `akua.toml` ↔ `akua.lock` integrity + cosign signatures +
	 * SLSA attestations. Exit `1` signals at least one violation in
	 * `out.violations`; the SDK returns the typed output either way.
	 */
	async verify(opts: VerifyOptions = {}): Promise<VerifyOutput> {
		const extra: string[] = [];
		if (opts.workspace) extra.push('--workspace', opts.workspace);
		if (opts.tarball) extra.push('--tarball', opts.tarball);
		if (opts.publicKey) extra.push('--public-key', opts.publicKey);
		return this.callDiagnostic<VerifyOutput>('verify', extra, 'VerifyOutput');
	}

	/**
	 * Structural diff between two directory trees of rendered
	 * manifests. JS side walks both trees + computes sha256 per
	 * file; akua-wasm compares the two `{path: hash}` manifests.
	 * No binary required.
	 */
	async diff(before: string, after: string): Promise<DirDiff> {
		const [beforeMap, afterMap] = await Promise.all([hashTree(before), hashTree(after)]);
		const wasm = await loadWasm();
		return validateAs<DirDiff>(
			'DirDiff',
			JSON.parse(wasm.diff(JSON.stringify(beforeMap), JSON.stringify(afterMap))),
		);
	}

	async render(opts: RenderOptions = {}): Promise<RenderSummary> {
		const args = ['render', '--json'];
		if (opts.package) args.push('--package', opts.package);
		if (opts.inputs) args.push('--inputs', opts.inputs);
		if (opts.out) args.push('--out', opts.out);
		if (opts.dryRun) args.push('--dry-run');
		if (opts.strict) args.push('--strict');
		if (opts.offline) args.push('--offline');
		const { stdout } = await this.exec(args);
		return validateAs<RenderSummary>('RenderSummary', JSON.parse(stdout));
	}

	private async call<T>(verb: string, schema: SchemaName): Promise<T> {
		const { stdout } = await this.exec([verb, '--json']);
		return validateAs<T>(schema, JSON.parse(stdout));
	}

	/**
	 * Variant of [`call`](Akua.call) for verbs that emit valid JSON on
	 * stdout regardless of exit code — `check`, `fmt --check`,
	 * `verify`, etc. — where the exit code is the *status signal*
	 * (`0 ok` vs `1 findings`) and both outcomes are data the caller
	 * wants. Only spawn-level failures (binary missing, OOM, signal)
	 * propagate.
	 */
	private async callDiagnostic<T>(verb: string, extraArgs: readonly string[], schema: SchemaName): Promise<T> {
		const { stdout } = await this.execTolerant([verb, '--json', ...extraArgs]);
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

	/**
	 * Like [`exec`](Akua.exec) but doesn't throw on non-zero exit as
	 * long as the process actually ran to completion. Spawn-level
	 * failures (binary missing, killed by signal) still throw via
	 * `classifyCliError`. Used by [`callDiagnostic`](Akua.callDiagnostic).
	 */
	private execTolerant(args: readonly string[]): Promise<{ stdout: string; stderr: string }> {
		return new Promise((resolve, reject) => {
			const child = execFile(
				this.binary,
				args,
				{ encoding: 'utf8', maxBuffer: DEFAULT_MAX_BUFFER },
				(err, stdout, stderr) => {
					if (!err || typeof child.exitCode === 'number') {
						resolve({ stdout, stderr });
						return;
					}
					reject(classifyCliError({ rawExitCode: child.exitCode, stderr, cause: err }));
				},
			);
		});
	}
}
