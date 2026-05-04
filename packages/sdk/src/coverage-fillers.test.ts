// Tests targeting the methods that were 0% covered prior to v0.8.4:
// version, whoami, inspect (package mode), tree, diff, verify, and
// the fmt stdout branch. Each test exercises both the SDK shim AND
// the matching napi binding, lifting coverage on both layers.
//
// Build fixtures via `scratchPackage` so we don't depend on the
// example tree's state. Keep each test self-contained.

import { describe, expect, test } from 'bun:test';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

import { Akua } from './mod.ts';
import { MINIMAL_PACKAGE_K, scratchPackage } from './test-utils.ts';

const akua = new Akua();

const MINIMAL_AKUA_TOML = `[package]
name = "coverage-test"
version = "0.0.1"
edition = "akua.dev/v1alpha1"
`;

function writeWorkspace(dir: string) {
	writeFileSync(join(dir, 'akua.toml'), MINIMAL_AKUA_TOML);
	writeFileSync(join(dir, 'package.k'), MINIMAL_PACKAGE_K);
}

describe('Akua.version', () => {
	test('returns a typed VersionOutput with the binary version', async () => {
		const v = await akua.version();
		expect(typeof v.version).toBe('string');
		// Don't pin the value — version moves; just shape.
		expect(v.version.length).toBeGreaterThan(0);
	});
});

describe('Akua.whoami', () => {
	test('returns agent_context with detected boolean and disabled_via_env', async () => {
		const w = await akua.whoami();
		expect(typeof w.agent_context.detected).toBe('boolean');
		expect(typeof w.agent_context.disabled_via_env).toBe('boolean');
		expect(typeof w.version).toBe('string');
	});
});

describe('Akua.tree', () => {
	test('zero-dep workspace returns an empty dependencies array', async () => {
		using ws = scratchPackage('akua-sdk-tree-');
		writeWorkspace(ws.dir);
		const t = await akua.tree({ workspace: ws.dir });
		expect(t.package.name).toBe('coverage-test');
		expect(t.package.version).toBe('0.0.1');
		expect(t.dependencies).toEqual([]);
	});

	test('path-dep surfaces with source = path', async () => {
		using ws = scratchPackage('akua-sdk-tree-dep-');
		// Sibling chart inside the workspace so the path-escape guard
		// doesn't fire.
		const chart = join(ws.dir, 'nginx-chart');
		mkdirSync(chart);
		mkdirSync(join(chart, 'templates'));
		writeFileSync(
			join(chart, 'Chart.yaml'),
			'apiVersion: v2\nname: nginx\nversion: 0.1.0\n',
		);
		writeFileSync(join(chart, 'templates/cm.yaml'), 'kind: ConfigMap\n');
		writeFileSync(
			join(ws.dir, 'akua.toml'),
			MINIMAL_AKUA_TOML +
				'[dependencies]\nnginx = { path = "./nginx-chart" }\n',
		);
		writeFileSync(join(ws.dir, 'package.k'), MINIMAL_PACKAGE_K);

		const t = await akua.tree({ workspace: ws.dir });
		const dep = t.dependencies.find((d) => d.name === 'nginx');
		expect(dep).toBeDefined();
		expect(dep?.source).toBe('path');
	});
});

describe('Akua.diff', () => {
	test('identical trees → no changes', async () => {
		using a = scratchPackage('akua-sdk-diff-a-');
		using b = scratchPackage('akua-sdk-diff-b-');
		writeFileSync(join(a.dir, 'app.yaml'), 'kind: Service\nname: x\n');
		writeFileSync(join(b.dir, 'app.yaml'), 'kind: Service\nname: x\n');
		const d = await akua.diff(a.dir, b.dir);
		expect(d.added).toEqual([]);
		expect(d.removed).toEqual([]);
		expect(d.changed).toEqual([]);
	});

	test('added/removed/changed each surface in the right bucket', async () => {
		using a = scratchPackage('akua-sdk-diff-a-');
		using b = scratchPackage('akua-sdk-diff-b-');
		writeFileSync(join(a.dir, 'gone.yaml'), 'old\n');
		writeFileSync(join(a.dir, 'modified.yaml'), 'before\n');
		writeFileSync(join(b.dir, 'modified.yaml'), 'after\n');
		writeFileSync(join(b.dir, 'new.yaml'), 'fresh\n');
		const d = await akua.diff(a.dir, b.dir);
		expect(d.added).toContain('new.yaml');
		expect(d.removed).toContain('gone.yaml');
		// `changed` is FileChange objects — assert on .path, with both
		// before/after sha256 surfaced.
		const modified = d.changed.find((c) => c.path === 'modified.yaml');
		expect(modified).toBeDefined();
		expect(modified?.before).toMatch(/^sha256:/);
		expect(modified?.after).toMatch(/^sha256:/);
		expect(modified?.before).not.toBe(modified?.after);
	});

	test('subdirectories are walked and reported with relative paths', async () => {
		using a = scratchPackage('akua-sdk-diff-nested-a-');
		using b = scratchPackage('akua-sdk-diff-nested-b-');
		mkdirSync(join(a.dir, 'sub'));
		mkdirSync(join(b.dir, 'sub'));
		writeFileSync(join(b.dir, 'sub/added.yaml'), 'x\n');
		const d = await akua.diff(a.dir, b.dir);
		// Directory separators are normalized by akua_core::dir_diff.
		expect(d.added.some((p) => p.includes('added.yaml'))).toBe(true);
	});
});

describe('Akua.verify', () => {
	test('workspace with akua.toml + akua.lock returns a verdict envelope', async () => {
		// Use an example workspace that already has a committed lockfile.
		// A scratch workspace would 404 on akua.lock since `akua lock`
		// hasn't been run.
		const ws = '../../examples/01-hello-webapp';
		const v = await akua.verify({ workspace: ws });
		expect(['ok', 'fail']).toContain(v.status);
		expect(v.summary).toBeDefined();
		expect(typeof v.summary.declared_deps).toBe('number');
		expect(typeof v.summary.locked_packages).toBe('number');
		expect(Array.isArray(v.violations)).toBe(true);
	});

	test('workspace without akua.lock surfaces E_LOCK_MISSING as a typed error', async () => {
		using ws = scratchPackage('akua-sdk-verify-no-lock-');
		writeWorkspace(ws.dir);
		// AkuaUserError carries the structured envelope — that's the
		// contract for typed-code routing on the consumer side.
		await expect(akua.verify({ workspace: ws.dir })).rejects.toMatchObject({
			structured: { code: 'E_LOCK_MISSING' },
		});
	});

	test('tarball mode is not implemented and throws', async () => {
		await expect(akua.verify({ tarball: '/tmp/nope.tar.gz' })).rejects.toThrow(
			/not yet implemented/,
		);
	});
});

describe('Akua.inspect (package mode)', () => {
	test('returns top-level options exposed via option(...) calls', async () => {
		using ws = scratchPackage('akua-sdk-inspect-');
		writeWorkspace(ws.dir);
		const r = await akua.inspect({ package: join(ws.dir, 'package.k') });
		expect(r.kind).toBe('package');
		// MINIMAL_PACKAGE_K calls `option("input")`, so the option list
		// surfaces a single top-level option named `input`. (Schema
		// fields like `replicas` are nested inside the Input schema
		// and not reported as separate top-level options.)
		expect(r.options).toBeDefined();
		expect(r.options?.some((o) => o.name === 'input')).toBe(true);
	});

	test('rejects when both package and tarball are passed', async () => {
		await expect(
			akua.inspect({ package: '/x/package.k', tarball: '/y.tar.gz' }),
		).rejects.toThrow(/not both/);
	});
});

describe('Akua.renderSource — path mode', () => {
	test('reads source from `package` path and renders', async () => {
		using ws = scratchPackage('akua-sdk-renderSource-path-');
		const pkg = join(ws.dir, 'package.k');
		writeFileSync(pkg, MINIMAL_PACKAGE_K);
		const yaml = await akua.renderSource({ package: pkg });
		expect(yaml).toContain('kind: ConfigMap');
		expect(yaml).toMatch(/count:\s*['"]?2['"]?/);
	});

	test('throws when neither source nor package is provided', async () => {
		// @ts-expect-error — deliberately wrong shape to hit the runtime guard
		await expect(akua.renderSource({})).rejects.toThrow(/source.*or.*package/);
	});
});

describe('Akua.fmt — stdout branch', () => {
	test('with stdout=true and a file that needs reformatting, writes to process.stdout', async () => {
		using ws = scratchPackage('akua-sdk-fmt-');
		// Write deliberately misformatted KCL so fmt reports `changed: true`.
		const pkg = join(ws.dir, 'package.k');
		writeFileSync(pkg, 'schema Input:\n  x:int=1\n');
		writeFileSync(join(ws.dir, 'akua.toml'), MINIMAL_AKUA_TOML);

		// Capture stdout while fmt runs. Bun gives us process.stdout.write
		// as the hook; intercept for one tick.
		const captured: string[] = [];
		const origWrite = process.stdout.write.bind(process.stdout);
		process.stdout.write = ((chunk: string | Uint8Array): boolean => {
			captured.push(typeof chunk === 'string' ? chunk : Buffer.from(chunk).toString());
			return true;
		}) as typeof process.stdout.write;

		try {
			const r = await akua.fmt({ package: pkg, stdout: true });
			expect(r.files[0]?.changed).toBe(true);
		} finally {
			process.stdout.write = origWrite;
		}

		expect(captured.join('').length).toBeGreaterThan(0);
	});

	test('with check=true the file is not modified and changed reflects diff', async () => {
		using ws = scratchPackage('akua-sdk-fmt-check-');
		const pkg = join(ws.dir, 'package.k');
		const original = 'schema Input:\n  x:int=1\n';
		writeFileSync(pkg, original);
		writeFileSync(join(ws.dir, 'akua.toml'), MINIMAL_AKUA_TOML);

		const r = await akua.fmt({ package: pkg, check: true });
		expect(r.files[0]?.changed).toBe(true);
		// File must not be touched in --check mode.
		const after = (await Bun.file(pkg).text()) as string;
		expect(after).toBe(original);
	});
});
