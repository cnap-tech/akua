// Structural diff between two rendered-output directories — what
// changed between yesterday's manifests and today's, useful for
// CI gates on Package upgrades. The JS side walks both trees and
// sha256s each file; the WASM bundle compares the two hash maps.

import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { Akua } from '../src/mod.ts';

const before = mkdtempSync(join(tmpdir(), 'akua-diff-before-'));
const after = mkdtempSync(join(tmpdir(), 'akua-diff-after-'));
try {
	mkdirSync(join(before, 'out'));
	mkdirSync(join(after, 'out'));
	writeFileSync(join(before, 'out', 'configmap.yaml'), 'kind: ConfigMap\ndata:\n  replicas: "2"\n');
	writeFileSync(join(before, 'out', 'removed.yaml'), 'kind: Service\n');
	writeFileSync(join(after, 'out', 'configmap.yaml'), 'kind: ConfigMap\ndata:\n  replicas: "7"\n');
	writeFileSync(join(after, 'out', 'added.yaml'), 'kind: Deployment\n');

	const akua = new Akua();
	const out = await akua.diff(join(before, 'out'), join(after, 'out'));

	console.log(`added   (${out.added.length}):`, out.added);
	console.log(`removed (${out.removed.length}):`, out.removed);
	console.log(`changed (${out.changed.length}):`);
	for (const c of out.changed) {
		console.log(`  ${c.path}`);
		console.log(`    before ${c.before}`);
		console.log(`    after  ${c.after}`);
	}
} finally {
	rmSync(before, { recursive: true, force: true });
	rmSync(after, { recursive: true, force: true });
}
