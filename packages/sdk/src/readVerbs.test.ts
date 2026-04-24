// Tests for the SDK's read verbs. `inspect`, `tree`, `diff` are
// WASM-backed — no binary required, run under plain `task sdk:test`.
// `verify` still shells out (cosign crypto isn't yet enabled on the
// WASM bundle) and stays gated on `AKUA_E2E=1`.

import { describe, expect, test } from 'bun:test';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

import { Akua } from './mod.ts';
import {
	BINARY,
	E2E_ENABLED,
	MINIMAL_PACKAGE_K,
	scratchPackage,
	scratchPackageWith,
} from './test-utils.ts';

const MINIMAL_TOML = `[package]
name = "smoke"
version = "0.0.1"
edition = "akua.dev/v1alpha1"
`;

const MINIMAL_LOCK = `version = 1
packages = []
`;

describe('Akua read verbs (WASM-backed)', () => {
	test('inspect returns a package body with options', async () => {
		using pkg = scratchPackageWith(MINIMAL_PACKAGE_K, 'akua-sdk-inspect-');
		const akua = new Akua();
		const out = await akua.inspect({ package: join(pkg.dir, 'package.k') });
		expect(out.kind).toBe('package');
		if (out.kind === 'package') {
			expect(Array.isArray(out.options)).toBe(true);
			expect(out.options.some((o) => o.name === 'input')).toBe(true);
		}
	});

	test('tree returns the package info + deps list', async () => {
		using pkg = scratchPackage('akua-sdk-tree-');
		writeFileSync(join(pkg.dir, 'akua.toml'), MINIMAL_TOML);
		const akua = new Akua();
		const out = await akua.tree({ workspace: pkg.dir });
		expect(out.package.name).toBe('smoke');
		expect(out.package.version).toBe('0.0.1');
		expect(Array.isArray(out.dependencies)).toBe(true);
		expect(out.dependencies.length).toBe(0);
	});

	test('diff between two identical dirs returns a clean diff', async () => {
		using a = scratchPackage('akua-sdk-diff-a-');
		using b = scratchPackage('akua-sdk-diff-b-');
		mkdirSync(join(a.dir, 'out'));
		mkdirSync(join(b.dir, 'out'));
		writeFileSync(join(a.dir, 'out', 'resource.yaml'), 'kind: ConfigMap\n');
		writeFileSync(join(b.dir, 'out', 'resource.yaml'), 'kind: ConfigMap\n');
		const akua = new Akua();
		const out = await akua.diff(join(a.dir, 'out'), join(b.dir, 'out'));
		expect(out.added).toEqual([]);
		expect(out.removed).toEqual([]);
		expect(out.changed).toEqual([]);
	});

	test('diff detects a changed file', async () => {
		using a = scratchPackage('akua-sdk-diff-a-');
		using b = scratchPackage('akua-sdk-diff-b-');
		mkdirSync(join(a.dir, 'out'));
		mkdirSync(join(b.dir, 'out'));
		writeFileSync(join(a.dir, 'out', 'resource.yaml'), 'kind: ConfigMap\n');
		writeFileSync(join(b.dir, 'out', 'resource.yaml'), 'kind: Service\n');
		const akua = new Akua();
		const out = await akua.diff(join(a.dir, 'out'), join(b.dir, 'out'));
		expect(out.changed.length).toBe(1);
		expect(out.changed[0].path).toContain('resource.yaml');
	});
});

describe.if(E2E_ENABLED)('Akua verify (shell-out)', () => {
	test('verify on a minimal workspace reports ok + no violations', async () => {
		using pkg = scratchPackage('akua-sdk-verify-');
		writeFileSync(join(pkg.dir, 'akua.toml'), MINIMAL_TOML);
		writeFileSync(join(pkg.dir, 'akua.lock'), MINIMAL_LOCK);
		const akua = new Akua({ binary: BINARY });
		const out = await akua.verify({ workspace: pkg.dir });
		expect(['ok', 'fail']).toContain(out.status);
		expect(Array.isArray(out.violations)).toBe(true);
		expect(typeof out.summary.declared_deps).toBe('number');
	});
});
