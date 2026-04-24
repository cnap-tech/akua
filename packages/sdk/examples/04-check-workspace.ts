// Run the three structural gates `akua check` uses: parse the
// manifest, parse the lockfile (if present), lint the Package.k.
// In-process via WASM; identical semantics to `akua check --json`.

import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { Akua } from '../src/mod.ts';

const MANIFEST = `[package]
name = "demo"
version = "0.1.0"
edition = "akua.dev/v1alpha1"
`;

const PACKAGE = `resources = []\n`;

const dir = mkdtempSync(join(tmpdir(), 'akua-check-example-'));
try {
	writeFileSync(join(dir, 'akua.toml'), MANIFEST);
	writeFileSync(join(dir, 'package.k'), PACKAGE);

	const akua = new Akua();
	const out = await akua.check({ workspace: dir });

	console.log(`status: ${out.status}`);
	for (const c of out.checks) {
		const marker = c.ok ? '✓' : '✗';
		console.log(`  ${marker} ${c.name}${c.error ? `: ${c.error}` : ''}`);
	}
} finally {
	rmSync(dir, { recursive: true, force: true });
}
