// Plugin-handler chart/overlay path resolution. Mirrors the simpler
// half of `crates/akua-core/src/kcl_plugin::resolve_in_package`:
// reject absolute paths, reject `..` escape from the package dir,
// require the resolved target to be an existing directory.
//
// The Rust path also walks symlinks via `canonicalize` and checks
// against an allowed-roots list. We don't need either yet — JS-side
// rendering is invoked from the SDK with a single concrete
// `packageDir`, no nested render scopes.

import { statSync } from 'node:fs';
import { isAbsolute, relative, resolve } from 'node:path';

/**
 * Resolve `relPath` against `packageDir`, validate the target is a
 * directory under `packageDir`, return its absolute path. Throws a
 * plugin-prefixed error on absolute path, escape, or non-directory.
 */
export function resolveDirInPackage(
	pluginName: string,
	packageDir: string,
	relPath: string,
	noun: 'chart' | 'overlay',
): string {
	if (isAbsolute(relPath)) {
		throw new Error(
			`${pluginName}: ${noun} path must be relative to the Package directory (got absolute: ${relPath})`,
		);
	}
	const abs = resolve(packageDir, relPath);
	const rel = relative(packageDir, abs);
	if (rel.startsWith('..')) {
		throw new Error(
			`${pluginName}: ${noun} path escapes Package directory: ${relPath} → ${abs}`,
		);
	}
	const stat = statSync(abs);
	if (!stat.isDirectory()) {
		throw new Error(`${pluginName}: ${noun} path is not a directory: ${abs}`);
	}
	return abs;
}
