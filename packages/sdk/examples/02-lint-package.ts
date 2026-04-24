// Lint a Package.k on disk. Runs the KCL parser in-process — no
// binary, no subprocess. Returns every parse-time issue with
// file + line + column anchors.

import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { Akua } from '../src/mod.ts';

// Write a Package.k with one deliberate unclosed list — KCL's
// parser catches it and reports `column` + `line`.
const dir = mkdtempSync(join(tmpdir(), 'akua-lint-example-'));
try {
	writeFileSync(
		join(dir, 'package.k'),
		'resources = [{ apiVersion: "v1", kind: "ConfigMap",\n',
	);

	const akua = new Akua();
	const out = await akua.lint({ package: join(dir, 'package.k') });

	console.log(`status: ${out.status}`);
	for (const issue of out.issues) {
		console.log(`  [${issue.level}] ${issue.code} ${issue.message}`);
		if (issue.file && issue.line) {
			console.log(`      at ${issue.file}:${issue.line}:${issue.column ?? 0}`);
		}
	}
} finally {
	rmSync(dir, { recursive: true, force: true });
}
