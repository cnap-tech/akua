// Loader stub for the `akua-napi` native addon. Resolves the
// `@akua-dev/native` per-platform binary that ships alongside
// `@akua-dev/sdk`, or — in the workspace dev build — the auto-
// generated `index.js` emitted by `napi build` at
// `crates/akua-napi/index.js`.
//
// The native addon is the SDK's transport for every verb that touches
// engines, OCI fetch, or cosign — same wasmtime + akua-core that
// `akua` (the binary) uses, in-process via Node-API. The wasm32-
// unknown-unknown bundle (`akua-wasm`) stays for browser use cases
// and the pure-KCL fast path.

import { createRequire } from 'node:module';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { existsSync } from 'node:fs';

import { type NapiStructuredError, parseNapiError } from './napi-error.ts';

/**
 * Mirror of the typed shape `crates/akua-napi/index.d.ts` exports.
 * Keep the field set in sync with the Rust `Napi*Args` structs;
 * unfortunately we can't import the generated `index.d.ts` directly
 * because it's auto-generated and may not exist at SDK type-check
 * time before the addon has been built.
 */
export interface NapiAddon {
	version(): unknown;
	render(args: {
		package: string;
		inputs?: string;
		out: string;
		dryRun?: boolean;
		strict?: boolean;
		offline?: boolean;
		timeout?: string;
		maxDepth?: number;
	}): unknown;
	renderToYaml(args: {
		package: string;
		inputs?: string;
		out: string;
		dryRun?: boolean;
		strict?: boolean;
		offline?: boolean;
		timeout?: string;
		maxDepth?: number;
	}): string;
	lint(args: { package: string }): unknown;
	fmt(args: { package: string; check?: boolean; stdout?: boolean }): unknown;
	check(args: { workspace: string; package?: string }): unknown;
	tree(args: { workspace: string }): unknown;
	diff(args: { before: string; after: string }): unknown;
	export(args: {
		package: string;
		format?: 'json-schema' | 'openapi';
		out?: string;
	}): unknown;
	verify(args: { workspace: string }): unknown;
	whoami(): unknown;
	inspect(args: { package?: string; tarball?: string }): unknown;
}

// Cache both successful loads AND failures. Without the failure
// cache, every call would retry the resolution chain forever — the
// real fix (install the binary, set NAPI_RS_NATIVE_LIBRARY_PATH)
// requires the consumer to take action, so failing fast on retries
// is the desired UX.
let cached: NapiAddon | undefined;
let cachedError: Error | undefined;

/**
 * Lazy-load and cache the napi addon for the host platform.
 * Resolution order matches what `@napi-rs/cli`'s generated stub does
 * (env override → co-located → scoped per-platform package), with a
 * dev-mode fallback to the workspace's `crates/akua-napi/index.js`.
 */
export function loadNapi(): NapiAddon {
	if (cached) return cached;
	if (cachedError) throw cachedError;
	const errors: string[] = [];
	const require_ = createRequire(import.meta.url);

	try {
		cached = require_('@akua-dev/native') as NapiAddon;
		return cached;
	} catch (err) {
		errors.push(`@akua-dev/native: ${(err as Error).message}`);
	}

	const here = dirname(fileURLToPath(import.meta.url));
	const dev = resolve(here, '../../../crates/akua-napi/index.js');
	if (existsSync(dev)) {
		cached = require_(dev) as NapiAddon;
		return cached;
	}
	errors.push(`workspace dev build: not at ${dev}`);

	cachedError = new Error(
		`@akua-dev/sdk: native addon not loadable. Tried:\n  - ${errors.join('\n  - ')}\n` +
			`Install via \`bun add @akua-dev/sdk\` (pulls the per-platform binary via optionalDependencies), ` +
			`or build locally with \`cd crates/akua-napi && bun run build\`.`,
	);
	throw cachedError;
}

/**
 * Wrap a napi addon call so the structured-error JSON the Rust side
 * embeds in the thrown error's message gets parsed and re-thrown as
 * the typed SDK error subclass — preserving the `code` /
 * `E_PACKAGE_MISSING`-style routing the CLI shell-out path used to
 * provide via stderr.
 */
export function callNapi<T>(invoke: () => unknown): T {
	try {
		return invoke() as T;
	} catch (err) {
		const structured = parseNapiError(err);
		if (structured) {
			throw structured;
		}
		throw err;
	}
}

export type { NapiStructuredError };
