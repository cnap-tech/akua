// Format a KCL source — `check` mode reports which files would
// change without touching disk; plain mode rewrites in place.
// In-process via WASM; no binary.

import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { Akua } from '../src/mod.ts';

// Intentionally ugly formatting — no trailing newline, extra
// whitespace. The KCL formatter cleans these up.
const UGLY = `resources=[{apiVersion:"v1",kind:"ConfigMap",metadata.name:"fmt-demo"}]`;

const dir = mkdtempSync(join(tmpdir(), 'akua-fmt-example-'));
const pkg = join(dir, 'package.k');
try {
	writeFileSync(pkg, UGLY);

	const akua = new Akua();

	// Check-mode: report the drift without touching disk
	const check = await akua.fmt({ package: pkg, check: true });
	console.log(`needs formatting: ${check.files[0].changed}`);
	// On-disk content unchanged after check-mode
	console.log(`on-disk bytes after check: ${readFileSync(pkg, 'utf8').length}`);

	// Now fix it in place
	await akua.fmt({ package: pkg });
	console.log('\n--- after fmt ---');
	console.log(readFileSync(pkg, 'utf8'));
} finally {
	rmSync(dir, { recursive: true, force: true });
}
