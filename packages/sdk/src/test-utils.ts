// Shared helpers for SDK tests. Kept thin — only what at least two
// test files use — to avoid every test pulling a giant utility
// surface. Not exported from `mod.ts`; tests import via the
// relative path.

import { existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

/**
 * Absolute path to the freshly-built `akua` binary. `task sdk:e2e`
 * runs `cargo build -p akua-cli` first and sets `AKUA_E2E=1`.
 */
export const BINARY = resolve(
	dirname(fileURLToPath(import.meta.url)),
	'../../../target/debug/akua',
);

/** True when the caller opted into end-to-end tests via `AKUA_E2E=1`. */
export const E2E_ENABLED = process.env.AKUA_E2E === '1';

/** Throws if the binary isn't present — prefer an explicit diagnostic
 * to the far-downstream `ENOENT` from `execFile`. */
export function assertBinaryBuilt(): void {
	if (!existsSync(BINARY)) {
		throw new Error(`missing: ${BINARY} — run \`task sdk:e2e\``);
	}
}

/**
 * Disposable scratch Package dir. Caller writes `package.k` via
 * `writeFileSync(join(pkg.dir, 'package.k'), ...)`, runs the render,
 * and relies on automatic cleanup via TC39 `using`:
 *
 * ```ts
 * using pkg = scratchPackage('akua-sdk-render-');
 * writeFileSync(join(pkg.dir, 'package.k'), PACKAGE_K);
 * await akua.render({ package: join(pkg.dir, 'package.k') });
 * // rmSync runs on scope exit.
 * ```
 */
export function scratchPackage(prefix = 'akua-sdk-'): { dir: string } & Disposable {
	const dir = mkdtempSync(join(tmpdir(), prefix));
	return {
		dir,
		[Symbol.dispose]() {
			rmSync(dir, { recursive: true, force: true });
		},
	};
}

/**
 * Convenience: same as [`scratchPackage`] but pre-writes a
 * `package.k` with the given KCL source. Test + fixture in one
 * line.
 */
export function scratchPackageWith(packageK: string, prefix = 'akua-sdk-'): { dir: string } & Disposable {
	const pkg = scratchPackage(prefix);
	writeFileSync(join(pkg.dir, 'package.k'), packageK);
	return pkg;
}

/**
 * Minimal pure-KCL Package emitting one ConfigMap whose `data.count`
 * echoes `input.replicas` (default 2). Reused by every in-process
 * render test that doesn't need engine callouts.
 */
export const MINIMAL_PACKAGE_K = `
schema Input:
    replicas: int = 2

input: Input = option("input") or Input {}

resources = [{
    apiVersion: "v1"
    kind: "ConfigMap"
    metadata.name: "smoke"
    data.count: str(input.replicas)
}]
`;
