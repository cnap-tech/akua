// Shared helpers for SDK tests. Kept thin — only what at least two
// test files use — to avoid every test pulling a giant utility
// surface. Not exported from `mod.ts`; tests import via the
// relative path.

import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

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
