// Unit tests for the synchronous gzip-tar writer in
// `src/wasi-host/tarball.ts`. The format mirrors what real Helm
// charts produce — long template paths under deep directory trees
// regularly exceed 100 bytes once the chart name prefix is added,
// so the USTAR `prefix` + `name` split has to work or every
// non-trivial chart fails the SDK render path.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { gunzipSync } from 'node:zlib';

import { tarGzipDir } from '../src/wasi-host/tarball.ts';

function makeChart(layout: Record<string, string>): string {
	const dir = mkdtempSync(join(tmpdir(), 'akua-tar-test-'));
	for (const [path, content] of Object.entries(layout)) {
		const abs = join(dir, path);
		mkdirSync(join(abs, '..'), { recursive: true });
		writeFileSync(abs, content);
	}
	return dir;
}

function listEntries(tarBytes: Uint8Array): { name: string; size: number }[] {
	// Trivial gnu/ustar reader for assertions only — extracts the
	// `prefix` + `name` fields and reconstructs the joined path.
	const decoder = new TextDecoder();
	const raw = gunzipSync(tarBytes);
	const out: { name: string; size: number }[] = [];
	let off = 0;
	while (off + 512 <= raw.length) {
		const block = raw.subarray(off, off + 512);
		// All-zero block marks end-of-archive.
		if (block.every((b) => b === 0)) break;
		const name = readField(block, 0, 100, decoder);
		const size = parseInt(readField(block, 124, 12, decoder).trim() || '0', 8);
		const prefix = readField(block, 345, 155, decoder);
		const joined = prefix ? `${prefix}/${name}` : name;
		out.push({ name: joined, size });
		off += 512 + Math.ceil(size / 512) * 512;
	}
	return out;
}

function readField(block: Uint8Array, offset: number, length: number, dec: TextDecoder): string {
	let end = offset;
	while (end < offset + length && block[end] !== 0) end += 1;
	return dec.decode(block.subarray(offset, end));
}

test('tarGzipDir handles a flat chart directory', () => {
	const dir = makeChart({
		'Chart.yaml': 'name: hello\n',
		'values.yaml': 'foo: bar\n',
	});
	try {
		const bytes = tarGzipDir(dir, 'hello');
		const entries = listEntries(bytes);
		const names = entries.map((e) => e.name).sort();
		assert.deepEqual(names, ['hello/', 'hello/Chart.yaml', 'hello/values.yaml']);
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
});

test('tarGzipDir splits paths >100 bytes via USTAR prefix field', () => {
	// Real-Helm shape: deep template tree under a chart name —
	// `<chart>/templates/<area>/<resource>.yaml` regularly tops 100
	// bytes once chart names get descriptive.
	const longSegment = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'; // 32 bytes
	const deepPath = `templates/${longSegment}/${longSegment}/${longSegment}/leaf.yaml`;
	const dir = makeChart({
		'Chart.yaml': 'name: deep\n',
		[deepPath]: 'apiVersion: v1\nkind: ConfigMap\n',
	});
	try {
		const bytes = tarGzipDir(dir, 'a-chart-with-a-name-of-its-own');
		const entries = listEntries(bytes);
		const fullPath = `a-chart-with-a-name-of-its-own/${deepPath}`;
		assert.ok(fullPath.length > 100, `precondition: path is >100 bytes (was ${fullPath.length})`);
		assert.ok(
			entries.some((e) => e.name === fullPath),
			`expected entry ${fullPath} in: ${entries.map((e) => e.name).join(', ')}`,
		);
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
});

test('tarGzipDir rejects single-segment paths >100 bytes (no PaxHeader yet)', () => {
	// 110-byte single segment — no `/` boundary leaves either side
	// small enough. PaxHeader fallback is out of scope; we throw a
	// clear error to keep the silent-fail surface small.
	const huge = 'b'.repeat(120);
	const dir = makeChart({ 'Chart.yaml': 'name: ok\n', [huge]: 'x' });
	try {
		assert.throws(() => tarGzipDir(dir, 'chart'), /USTAR/);
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
});
