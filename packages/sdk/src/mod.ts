import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, dirname, resolve as resolvePath, join as joinPath } from 'node:path';

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

// Every verb is dispatched through the napi addon. We previously
// kept a wasm-bindgen path for "pure" verbs (check, fmt, lint, tree,
// diff, export, inspect-as-package) on the theory that those methods
// could run in the browser. The SDK package is published Node-only
// (engines.node>=22 + a hard dep on @akua-dev/native), so that
// optionality bought nothing — and the wasm path bundled the wasm-
// pack wrapper's `__dirname` as a build-time absolute path that
// broke at runtime. Dropping the second transport eliminates that
// bug class entirely. If browser support comes back, it ships as a
// separate `@akua-dev/sdk-wasm` package, not as a side-channel here.

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
	/**
	 * Wall-clock cap on the render. Go duration string —
	 * `30s`, `5m`, `1h`, `250ms`. Mirrors the universal
	 * `--timeout` flag; nested `pkg.render` calls inherit it.
	 */
	timeout?: string;
	/**
	 * Hard cap on `pkg.render` composition depth (default 16).
	 * Hitting the cap surfaces `E_RENDER_BUDGET_DEPTH`. Pair with
	 * `timeout` to harden CI / agent renders against runaways.
	 */
	maxDepth?: number;
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
	/**
	 * Wall-clock cap (Go duration). Same semantics as
	 * [`RenderOptions.timeout`].
	 */
	timeout?: string;
	/**
	 * `pkg.render` composition depth cap. See
	 * [`RenderOptions.maxDepth`].
	 */
	maxDepth?: number;
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
	 * rendered top-level YAML. Runs in-process via the napi addon.
	 * Helm + Kustomize engine callouts (`helm.template`,
	 * `kustomize.build`) resolve against the package's directory.
	 */
	async renderSource(opts: RenderSourceOptions): Promise<string> {
		let source: string;
		let packageFilename: string;
		if (opts.source !== undefined) {
			source = opts.source;
			packageFilename = opts.packageFilename ?? 'package.k';
		} else if (opts.package !== undefined) {
			source = await readFile(resolvePath(opts.package), 'utf8');
			packageFilename = opts.packageFilename ?? basename(opts.package);
		} else {
			throw new Error('renderSource: provide either `source` or `package`');
		}

		const napi = loadNapi();
		// `napi.renderToYaml` mirrors `akua render --stdout` — emits
		// raw multi-doc YAML directly. When the caller hands us raw
		// source we materialize it into a scratch dir so KCL spans +
		// chart-path resolution work the same as a path-mode render.
		// Otherwise we use the caller's path verbatim. `packageDir`
		// is implicit (dirname of `sourcePath`).
		const tmp = await mkdtemp(joinPath(tmpdir(), 'akua-sdk-render-'));
		const sourcePath =
			opts.package !== undefined ? resolvePath(opts.package) : joinPath(tmp, packageFilename);
		// `napi.renderToYaml` reads `inputs` as a filesystem path, so
		// stage inline values into a sibling file before invoking it.
		let inputsPath: string | undefined;
		if (opts.inputs !== undefined) {
			inputsPath = joinPath(tmp, 'inputs.json');
			await writeFile(inputsPath, JSON.stringify(opts.inputs), 'utf8');
		}
		try {
			if (opts.package === undefined) {
				await writeFile(sourcePath, source, 'utf8');
			}
			return callNapi<string>(() =>
				napi.renderToYaml({
					package: sourcePath,
					// `out` is unused in stdout-mode (no files written) but
					// the verb arg-shape requires it.
					out: joinPath(tmp, 'unused'),
					inputs: inputsPath,
					timeout: opts.timeout,
					maxDepth: opts.maxDepth,
				}),
			);
		} finally {
			await rm(tmp, { recursive: true, force: true });
		}
	}

	/**
	 * Fast syntax / type / dep check over the workspace. Runs
	 * in-process via the napi addon. Mirrors `akua check`.
	 */
	async check(opts: CheckOptions = {}): Promise<CheckOutput> {
		const napi = loadNapi();
		const result = callNapi<unknown>(() =>
			napi.check({ workspace: opts.workspace ?? '.', package: opts.package }),
		);
		return validateAs<CheckOutput>('CheckOutput', result);
	}

	/**
	 * Run the KCL linter against the Package. In-process via the
	 * napi addon. Mirrors `akua lint`.
	 */
	async lint(opts: LintOptions = {}): Promise<LintOutput> {
		const napi = loadNapi();
		const result = callNapi<unknown>(() =>
			napi.lint({ package: opts.package ?? './package.k' }),
		);
		return validateAs<LintOutput>('LintOutput', result);
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
		const napi = loadNapi();
		// napi.export returns the verb's `{format, schema}` envelope;
		// the SDK contract has always been the bare schema document
		// (the WASM path called `export_input_schema` / `_openapi`
		// directly, which return just the schema). Unwrap.
		const envelope = callNapi<{ format: string; schema: Record<string, unknown> }>(() =>
			napi.export({
				package: opts.package ?? './package.k',
				format: opts.format ?? 'json-schema',
			}),
		);
		return envelope.schema;
	}

	/**
	 * Format KCL sources. In-process via the napi addon.
	 * With `check=true`, reports which files would change without
	 * touching disk. Without `check`, the formatted text is written
	 * back to the file (mirroring `akua fmt`'s in-place behavior).
	 *
	 * `opts.stdout` is honored by reading the (now-formatted) file
	 * and writing it to `process.stdout`. The file write happens
	 * either way because napi's fmt verb performs the write before
	 * returning; restoring the original on stdout-mode would race
	 * with concurrent readers.
	 */
	async fmt(opts: FmtOptions = {}): Promise<FmtOutput> {
		const pkg = opts.package ?? './package.k';
		const napi = loadNapi();
		const result = callNapi<{ files: FmtFile[] }>(() =>
			napi.fmt({ package: pkg, check: opts.check ?? false }),
		);
		if (opts.stdout && !opts.check && (result.files[0]?.changed ?? false)) {
			process.stdout.write(await readFile(pkg, 'utf8'));
		}
		return validateAs<FmtOutput>('FmtOutput', result);
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
		const napi = loadNapi();
		const result = callNapi<unknown>(() =>
			napi.inspect(
				opts.tarball ? { tarball: opts.tarball } : { package: opts.package ?? './package.k' },
			),
		);
		return validateAs<InspectOutput>('InspectOutput', result);
	}

	/**
	 * Print the workspace's declared deps + lockfile entries.
	 * In-process via the napi addon.
	 */
	async tree(opts: TreeOptions = {}): Promise<TreeOutput> {
		const napi = loadNapi();
		const result = callNapi<unknown>(() => napi.tree({ workspace: opts.workspace ?? '.' }));
		return validateAs<TreeOutput>('TreeOutput', result);
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
	 * manifests. The napi addon walks both trees server-side and
	 * compares hashes; no JS-side hashing.
	 */
	async diff(before: string, after: string): Promise<DirDiff> {
		const napi = loadNapi();
		const result = callNapi<unknown>(() => napi.diff({ before, after }));
		return validateAs<DirDiff>('DirDiff', result);
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
				timeout: opts.timeout,
				maxDepth: opts.maxDepth,
			}),
		);
		return validateAs<RenderSummary>('RenderSummary', result);
	}

}
