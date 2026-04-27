// Tests for the CI-gate trio: `check`, `lint`, `fmt`. WASM-backed —
// run under plain `task sdk:test`, no binary required.

import { describe, expect, test } from 'bun:test';
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';

import { Akua } from './mod.ts';
import { MINIMAL_PACKAGE_K, scratchPackageWith } from './test-utils.ts';

const MINIMAL_TOML = `[package]
name = "smoke"
version = "0.0.1"
edition = "akua.dev/v1alpha1"
`;

describe('Akua CI-gate verbs', () => {
	test('check returns a CheckOutput with a "manifest" entry', async () => {
		using pkg = scratchPackageWith(MINIMAL_PACKAGE_K, 'akua-sdk-check-');
		writeFileSync(join(pkg.dir, 'akua.toml'), MINIMAL_TOML);
		const akua = new Akua();
		const out = await akua.check({
			workspace: pkg.dir,
			package: join(pkg.dir, 'package.k'),
		});
		expect(['ok', 'fail']).toContain(out.status);
		expect(out.checks.some((c) => c.name === 'manifest')).toBe(true);
	});

	test('lint returns a LintOutput with an issues array', async () => {
		using pkg = scratchPackageWith(MINIMAL_PACKAGE_K, 'akua-sdk-lint-');
		const akua = new Akua();
		const out = await akua.lint({ package: join(pkg.dir, 'package.k') });
		expect(['ok', 'fail']).toContain(out.status);
		expect(Array.isArray(out.issues)).toBe(true);
	});

	test('fmt --check reports whether formatting would change the file', async () => {
		using pkg = scratchPackageWith(MINIMAL_PACKAGE_K, 'akua-sdk-fmt-');
		const akua = new Akua();
		const out = await akua.fmt({
			package: join(pkg.dir, 'package.k'),
			check: true,
		});
		expect(out.files.length).toBe(1);
		expect(typeof out.files[0].changed).toBe('boolean');
	});

	test('export emits JSON Schema 2020-12 with @ui decorators projected to x-ui', async () => {
		const PKG_WITH_UI = `
schema Input:
    """Public inputs."""

    @ui(order=10, group="Identity")
    name: str = "hello"

    replicas: int = 2

resources = []
`;
		using pkg = scratchPackageWith(PKG_WITH_UI, 'akua-sdk-export-');
		const akua = new Akua();
		const schema = await akua.export({ package: join(pkg.dir, 'package.k') });
		expect(schema.$schema).toBe('https://json-schema.org/draft/2020-12/schema');
		const props = schema.properties as Record<string, Record<string, unknown>>;
		expect(props.name.type).toBe('string');
		const xUi = props.name['x-ui'] as Record<string, unknown>;
		expect(xUi.order).toBe(10);
		expect(xUi.group).toBe('Identity');
	});

	test('export with format=openapi wraps Input under components.schemas', async () => {
		using pkg = scratchPackageWith(MINIMAL_PACKAGE_K, 'akua-sdk-export-openapi-');
		const akua = new Akua();
		const doc = await akua.export({
			package: join(pkg.dir, 'package.k'),
			format: 'openapi',
		});
		expect(doc.openapi).toBe('3.1.0');
		const components = doc.components as Record<string, Record<string, unknown>>;
		expect(typeof components.schemas).toBe('object');
		expect(components.schemas.Input).toBeDefined();
	});
});
