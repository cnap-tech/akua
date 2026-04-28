import { createHash } from 'node:crypto';
import { readFile, readdir, stat, writeFile } from 'node:fs/promises';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, dirname, posix as posixPath, resolve as resolvePath, join as joinPath } from 'node:path';

import { callNapi, loadNapi } from './napi.ts';

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
	export_input_schema: (filename: string, source: string) => string;
	export_input_openapi: (filename: string, source: string) => string;
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
				const absPath = posixPath.join(dir, entry.name);
				const relPath = rel ? posixPath.join(rel, entry.name) : entry.name;
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
export type { StructuredError } from './types/StructuredError.ts';
export type { Level } from './types/Level.ts';
export type { AgentContext } from './types/AgentContext.ts';
export type { AgentSource } from './types/AgentSource.ts';

export interface AkuaOptions {
	/**
	 * Reserved for future configuration knobs (cache dir, log
	 * threshold, etc.). Currently empty — every method routes through
	 * the bundled native addon (`@akua-dev/native` per platform), so
	 * there's no binary path to override.
	 */
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

export interface ExportOptions {
	/** Path to the `package.k` file. Default: `./package.k`. */
	package?: string;
	/**
	 * Output format. `json-schema` (default) emits raw JSON Schema 2020-12;
	 * `openapi` wraps it as an OpenAPI 3.1 doc with the `Input` schema
	 * under `components.schemas.Input`.
	 */
	format?: 'json-schema' | 'openapi';
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

export interface RenderSourceOptions {
	/**
	 * Raw KCL Package source. Mutually exclusive with `package`.
	 */
	source?: string;
	/** Path to the Package.k on disk. */
	package?: string;
	/**
	 * Diagnostic filename — only used when `source` is provided
	 * directly (KCL surfaces it in error spans). Default `package.k`.
	 */
	packageFilename?: string;
	/**
	 * Workspace directory for resolving relative `helm.template` /
	 * `kustomize.build` paths. Defaults to `dirname(package)` when
	 * `package` is provided; required if the source uses either
	 * plugin and `source` is given directly.
	 */
	packageDir?: string;
	/**
	 * Inputs to inject as KCL's `option("input")`. Pass any
	 * JSON-serializable value, or omit for an empty mapping.
	 */
	inputs?: unknown;
}

/**
 * Cheap probe: does the Package source mention either engine plugin?
 * Substring scan; false positives just route to the WASI worker
 * unnecessarily (correct, just slower). False negatives surface as
 * KCL "no plugin registered under …" errors.
 */
function needsEngineHost(source: string): boolean {
	return source.includes('helm.') || source.includes('kustomize.');
}

/**
 * Thin wrapper around the `akua` CLI. Each method shells out to a verb,
 * parses the `--json` output, and returns a value typed by the ts-rs
 * generated types. Failures throw the right `AkuaError` subclass based
 * on exit code + parsed StructuredError.
 */
export class Akua {
	constructor(_opts: AkuaOptions = {}) {
		// All transport now goes through the native napi addon; no
		// per-instance state. Keep the constructor for backwards
		// compat with callers using `new Akua()`.
	}

	async version(): Promise<VersionOutput> {
		const napi = loadNapi();
		return validateAs<VersionOutput>('VersionOutput', callNapi(() => napi.version()));
	}

	async whoami(): Promise<WhoamiOutput> {
		const napi = loadNapi();
		return validateAs<WhoamiOutput>('WhoamiOutput', callNapi(() => napi.whoami()));
	}

	/**
	 * Evaluate a Package against optional inputs and return the
	 * rendered top-level YAML. Runs entirely in-process via the
	 * sandboxed `akua-render-worker.wasm` hosted under Node WASI —
	 * no `akua` binary required. Helm + Kustomize engine callouts
	 * (`helm.template`, `kustomize.build`) work transparently when
	 * `packageDir` is provided so relative chart / overlay paths
	 * resolve.
	 *
	 * Two argument shapes:
	 * - **Object form (preferred):** `renderSource({ package, source,
	 *   inputs, packageFilename, packageDir })`. Mix `package` (path
	 *   on disk) + `inputs` together for the common case.
	 * - **Legacy form:** `renderSource(packageFilename, source,
	 *   inputs)` — kept for backwards-compat with the pre-engine
	 *   pure-KCL helper. Cannot resolve `helm.template` paths;
	 *   prefer the object form for new code.
	 */
	renderSource(opts: RenderSourceOptions): Promise<string>;
	renderSource(packageFilename: string, source: string, inputs?: unknown): Promise<string>;
	async renderSource(
		optsOrFilename: RenderSourceOptions | string,
		legacySource?: string,
		legacyInputs?: unknown,
	): Promise<string> {
		const opts =
			typeof optsOrFilename === 'string'
				? {
						source: legacySource ?? '',
						packageFilename: optsOrFilename,
						inputs: legacyInputs,
					}
				: optsOrFilename;

		let source: string;
		let packageFilename: string;
		let packageDir: string | undefined;
		if (opts.source !== undefined) {
			source = opts.source;
			packageFilename = opts.packageFilename ?? 'package.k';
			packageDir = opts.packageDir;
		} else if (opts.package !== undefined) {
			const abs = resolvePath(opts.package);
			source = await readFile(abs, 'utf8');
			packageFilename = opts.packageFilename ?? basename(abs);
			packageDir = opts.packageDir ?? dirname(abs);
		} else {
			throw new Error('renderSource: provide either `source` or `package`');
		}

		// Dispatch:
		// - Pure-KCL goes through `akua-wasm` (~30ms warm, runtime-
		//   portable: Node, Bun, browsers, no native binary required).
		// - Packages that invoke `helm.template` / `kustomize.build`
		//   route through the napi addon. `napi.render` takes a
		//   filesystem path — when the caller passes raw `source`, we
		//   write it to a tempdir alongside the requested
		//   `packageFilename` so KCL diagnostics still reference the
		//   intended span. The temp directory becomes the implicit
		//   `packageDir` for chart-path resolution unless the caller
		//   set one explicitly.
		if (needsEngineHost(source)) {
			const napi = loadNapi();
			// `napi.renderToYaml` mirrors `akua render --stdout` — emits
			// raw multi-doc YAML instead of writing files + returning a
			// summary. The caller wants the YAML bytes directly.
			//
			// When the caller hands us raw source we materialize it
			// into a scratch dir so KCL spans + chart-path resolution
			// work the same as a path-mode render. Otherwise we use the
			// caller's path verbatim.
			const tmp = await mkdtemp(joinPath(tmpdir(), 'akua-sdk-render-'));
			const sourcePath =
				opts.package !== undefined ? resolvePath(opts.package) : joinPath(tmp, packageFilename);
			try {
				if (opts.package === undefined) {
					await writeFile(sourcePath, source, 'utf8');
				}
				return callNapi<string>(() =>
					napi.renderToYaml({
						package: sourcePath,
						// `out` is unused in stdout-mode (no files
						// written) but the verb arg-shape requires it.
						out: joinPath(tmp, 'unused'),
					}),
				);
			} finally {
				await rm(tmp, { recursive: true, force: true });
			}
		}

		const wasm = await loadWasm();
		const inputsJson = opts.inputs === undefined ? null : JSON.stringify(opts.inputs);
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
		const manifestPath = posixPath.join(ws, 'akua.toml');
		const lockPath = posixPath.join(ws, 'akua.lock');
		const pkgPath = opts.package ?? posixPath.join(ws, 'package.k');

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
	 * Emit the Package's `Input` schema as JSON Schema 2020-12 (default)
	 * or OpenAPI 3.1. In-process via WASM — no binary required. Returns
	 * the parsed schema document; consumers feed it to UI form renderers
	 * (rjsf, JSONForms), API doc tools, or admission-webhook validators.
	 *
	 * Field-level docstrings become `description`; `@ui(...)` decorators
	 * become OpenAPI-3.1-compliant `x-ui` extensions. See
	 * [`docs/cli.md`](https://github.com/cnap-tech/akua/blob/main/docs/cli.md#akua-export)
	 * for the full schema contract.
	 */
	async export(opts: ExportOptions = {}): Promise<Record<string, unknown>> {
		const pkg = opts.package ?? './package.k';
		const source = await readFile(pkg, 'utf8');
		const wasm = await loadWasm();
		const raw =
			opts.format === 'openapi'
				? wasm.export_input_openapi(pkg, source)
				: wasm.export_input_schema(pkg, source);
		return JSON.parse(raw) as Record<string, unknown>;
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
	 * Introspect a Package or a packed tarball — surfaces the option
	 * set, dependency tree, signing metadata. Pass `{ package }` for
	 * an on-disk Package or `{ tarball }` for a `.tar.gz` artifact
	 * (e.g. from `akua pack`).
	 */
	async inspect(opts: InspectOptions = {}): Promise<InspectOutput> {
		if (opts.package && opts.tarball) {
			throw new Error('inspect: pass either `package` or `tarball`, not both');
		}
		// Tarball mode requires the napi addon (tar reader + cosign +
		// OCI manifest parsing). Package mode is pure-KCL and stays on
		// the akua-wasm fast path so it works on browsers + Bun
		// without the native binary loaded.
		if (opts.tarball) {
			const napi = loadNapi();
			const result = callNapi<unknown>(() => napi.inspect({ tarball: opts.tarball }));
			return validateAs<InspectOutput>('InspectOutput', result);
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
			readFile(posixPath.join(ws, 'akua.toml'), 'utf8'),
			readOptional(posixPath.join(ws, 'akua.lock')),
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
		if (opts.tarball) {
			throw new Error(
				'verify({ tarball }) is not yet implemented in the SDK — pass `workspace` instead, or use the CLI for tarball verification.',
			);
		}
		const napi = loadNapi();
		const result = callNapi<unknown>(() =>
			napi.verify({ workspace: opts.workspace ?? '.' }),
		);
		return validateAs<VerifyOutput>('VerifyOutput', result);
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
		const napi = loadNapi();
		const result = callNapi<unknown>(() =>
			napi.render({
				package: opts.package ?? './package.k',
				inputs: opts.inputs,
				out: opts.out ?? './deploy',
				dryRun: opts.dryRun,
				strict: opts.strict,
				offline: opts.offline,
			}),
		);
		return validateAs<RenderSummary>('RenderSummary', result);
	}

}
