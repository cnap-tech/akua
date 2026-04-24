// Introspect a Package's `option()` call-sites without executing
// the program — what inputs does this package consume, with what
// types and defaults? Useful for building a UI on top of a
// Package, or for validating a deployment's input values against
// the schema.

import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { Akua } from '../src/mod.ts';

const PACKAGE = `
schema Input:
    appName: str
    replicas: int = 2
    hostname?: str

input: Input = option("input") or Input {}

resources = []
`;

const dir = mkdtempSync(join(tmpdir(), 'akua-inspect-example-'));
try {
	writeFileSync(join(dir, 'package.k'), PACKAGE);

	const akua = new Akua();
	const out = await akua.inspect({ package: join(dir, 'package.k') });

	if (out.kind !== 'package') throw new Error('expected package body');
	console.log(`options found in ${out.path}:`);
	for (const opt of out.options) {
		const req = opt.required ? '' : ' (optional)';
		const def = opt.default ? ` = ${opt.default}` : '';
		console.log(`  ${opt.name}: ${opt.type || 'any'}${def}${req}`);
	}
} finally {
	rmSync(dir, { recursive: true, force: true });
}
