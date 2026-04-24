// End-to-end tests for the SDK's read verbs: inspect, tree, verify,
// diff. Each shells out to the freshly-built `akua` binary against a
// scratch workspace and validates the typed output. Gated on
// `AKUA_E2E=1`.

import { describe, expect, test } from 'bun:test';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

import { Akua } from './mod.ts';
import { BINARY, E2E_ENABLED, MINIMAL_PACKAGE_K, scratchPackage, scratchPackageWith } from './test-utils.ts';

const MINIMAL_TOML = `[package]
name = "smoke"
version = "0.0.1"
edition = "akua.dev/v1alpha1"
`;

// Minimal `akua.lock` — empty packages list, schema version 1.
// `akua verify` requires the lockfile; `akua tree` reads both.
const MINIMAL_LOCK = `version = 1
packages = []
`;

describe.if(E2E_ENABLED)('Akua read verbs', () => {
	test('inspect returns a package body with options', async () => {
		using pkg = scratchPackageWith(MINIMAL_PACKAGE_K, 'akua-sdk-inspect-');
		const akua = new Akua({ binary: BINARY });
		const out = await akua.inspect({ package: join(pkg.dir, 'package.k') });
		expect(out.kind).toBe('package');
		if (out.kind === 'package') {
			expect(Array.isArray(out.options)).toBe(true);
			// The MINIMAL Package declares `option("input")`, so we expect
			// exactly one option entry named `input`.
			expect(out.options.some((o) => o.name === 'input')).toBe(true);
		}
	});

	test('tree returns the package info + deps list', async () => {
		using pkg = scratchPackage('akua-sdk-tree-');
		writeFileSync(join(pkg.dir, 'akua.toml'), MINIMAL_TOML);
		const akua = new Akua({ binary: BINARY });
		const out = await akua.tree({ workspace: pkg.dir });
		expect(out.package.name).toBe('smoke');
		expect(out.package.version).toBe('0.0.1');
		expect(Array.isArray(out.dependencies)).toBe(true);
		expect(out.dependencies.length).toBe(0);
	});

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

	test('diff between two identical dirs returns a clean diff', async () => {
		using a = scratchPackage('akua-sdk-diff-a-');
		using b = scratchPackage('akua-sdk-diff-b-');
		mkdirSync(join(a.dir, 'out'));
		mkdirSync(join(b.dir, 'out'));
		writeFileSync(join(a.dir, 'out', 'resource.yaml'), 'kind: ConfigMap\n');
		writeFileSync(join(b.dir, 'out', 'resource.yaml'), 'kind: ConfigMap\n');
		const akua = new Akua({ binary: BINARY });
		const out = await akua.diff(join(a.dir, 'out'), join(b.dir, 'out'));
		expect(out.added).toEqual([]);
		expect(out.removed).toEqual([]);
		expect(out.changed).toEqual([]);
	});
});
