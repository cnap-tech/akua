// Synchronous gzip-tar of a directory tree.
//
// Why sync: the plugin bridge from the worker (`kcl_plugin_invoke_
// json_wasm` import) must return synchronously inside `wasi.start()`.
// Async tarballers (npm `tar`, `tar-stream`) yield to the event
// loop; we can't `await` from inside a sync wasm import. Tar's
// on-disk format is simple enough that a 512-byte-header writer
// fits in ~120 LoC and beats the worker-thread + SharedArrayBuffer
// alternative for clarity.
//
// Format: gnu/ustar variant — what `archive/tar` (Go) and `tar::
// Builder::append_dir_all` (Rust) produce. The helm/kustomize engines
// consume this without complaint.

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { posix as posixPath, sep as platformSep } from 'node:path';
import { gzipSync } from 'node:zlib';

const BLOCK_SIZE = 512;
const NAME_FIELD = 100;
const PREFIX_FIELD = 155;

type Entry = {
	path: string; // archive-relative, with trailing `/` for dirs
	type: 'file' | 'dir';
	mode: number;
	mtime: number;
	content: Uint8Array;
};

/**
 * Tar `dir` into a gzip'd buffer; archive entries are prefixed with
 * `<nameInArchive>/`. Mirrors the Rust shape:
 *
 * ```
 * tar::Builder::append_dir_all(name_in_archive, dir)
 * ```
 */
export function tarGzipDir(dir: string, nameInArchive: string): Uint8Array {
	const entries: Entry[] = [];
	collectEntries(dir, nameInArchive, entries);
	// Sort for deterministic output (helm/kustomize engines don't
	// require it, but matching the Rust path makes byte-diff tests
	// possible later).
	entries.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));

	const blocks: Uint8Array[] = [];
	for (const entry of entries) {
		blocks.push(headerBlock(entry));
		if (entry.type === 'file' && entry.content.length > 0) {
			blocks.push(entry.content);
			const pad = (BLOCK_SIZE - (entry.content.length % BLOCK_SIZE)) % BLOCK_SIZE;
			if (pad > 0) blocks.push(new Uint8Array(pad));
		}
	}
	// Two zero blocks mark the archive end.
	blocks.push(new Uint8Array(BLOCK_SIZE * 2));

	const total = blocks.reduce((n, b) => n + b.length, 0);
	const tar = new Uint8Array(total);
	let off = 0;
	for (const b of blocks) {
		tar.set(b, off);
		off += b.length;
	}
	const gz = gzipSync(tar);
	return new Uint8Array(gz.buffer, gz.byteOffset, gz.byteLength);
}

function collectEntries(dir: string, archivePath: string, out: Entry[]): void {
	const dirStat = statSync(dir);
	out.push({
		path: ensureTrailingSlash(archivePath),
		type: 'dir',
		mode: dirStat.mode & 0o7777,
		mtime: Math.floor(dirStat.mtimeMs / 1000),
		content: new Uint8Array(0),
	});

	for (const child of readdirSync(dir, { withFileTypes: true })) {
		if (child.isSymbolicLink()) {
			// Mirrors `tar.follow_symlinks(false)` on the Rust side —
			// symlinks would broaden the trust boundary, skip them.
			continue;
		}
		const childOnDisk = `${dir}${platformSep}${child.name}`;
		const childArchive = posixPath.join(archivePath, child.name);
		if (child.isDirectory()) {
			collectEntries(childOnDisk, childArchive, out);
		} else if (child.isFile()) {
			const content = readFileSync(childOnDisk);
			const stat = statSync(childOnDisk);
			out.push({
				path: childArchive,
				type: 'file',
				mode: stat.mode & 0o7777,
				mtime: Math.floor(stat.mtimeMs / 1000),
				content: new Uint8Array(content.buffer, content.byteOffset, content.byteLength),
			});
		}
	}
}

function ensureTrailingSlash(p: string): string {
	return p.endsWith('/') ? p : `${p}/`;
}

// --- header writer --------------------------------------------------------

function headerBlock(entry: Entry): Uint8Array {
	const block = new Uint8Array(BLOCK_SIZE);
	const enc = new TextEncoder();
	const nameBytes = enc.encode(entry.path);

	// USTAR encoding: 100-byte `name` field + 155-byte `prefix` field,
	// joined by `/` on read. Real Helm charts hit paths like
	// `<chart>/templates/foo/bar/baz.yaml` that exceed 100 bytes —
	// USTAR's prefix+name split covers up to ~255 bytes, fine for
	// every chart we care about. PaxHeader (the GNU extension for
	// >255-byte paths) isn't worth implementing yet; throw with a
	// clear error for the rare overshoot.
	if (nameBytes.length <= NAME_FIELD) {
		block.set(nameBytes, 0);
	} else {
		const split = splitPathForUstar(entry.path);
		if (!split) {
			throw new Error(
				`tarball: entry path too long for USTAR (${nameBytes.length} bytes; max ~255 with prefix split, no '/' boundary fits): ${entry.path}`,
			);
		}
		const prefixBytes = enc.encode(split.prefix);
		const tailBytes = enc.encode(split.name);
		block.set(tailBytes, 0);
		block.set(prefixBytes, 345); // USTAR prefix offset
	}

	writeOctal(block, 100, 8, entry.mode); // mode
	writeOctal(block, 108, 8, 0); // uid
	writeOctal(block, 116, 8, 0); // gid
	writeOctal(block, 124, 12, entry.type === 'file' ? entry.content.length : 0); // size
	writeOctal(block, 136, 12, entry.mtime); // mtime
	// Checksum field starts as 8 spaces for the sum calculation.
	for (let i = 148; i < 156; i++) block[i] = 0x20;

	block[156] = entry.type === 'dir' ? 0x35 /* '5' */ : 0x30 /* '0' */; // typeflag

	// linkname [157..257]: zero
	block.set(enc.encode('ustar\0'), 257); // magic
	block.set(enc.encode('00'), 263); // version
	// uname/gname/devmajor/devminor/prefix all zero — fine for our use.

	let sum = 0;
	for (let i = 0; i < BLOCK_SIZE; i++) sum += block[i];
	writeOctal(block, 148, 7, sum);
	block[155] = 0; // NUL after checksum
	return block;
}

/**
 * Split a path into USTAR `prefix` + `name` so that `prefix` fits in
 * 155 bytes, `name` fits in 100, and the split is on a `/` boundary.
 * Returns `null` when no such split exists (single-segment path
 * over 100 bytes, or no boundary leaves either side small enough).
 *
 * Strategy: walk separators right-to-left, take the rightmost split
 * whose tail still fits in 100 bytes.
 */
function splitPathForUstar(path: string): { prefix: string; name: string } | null {
	const enc = new TextEncoder();
	for (let i = path.length - 1; i > 0; i--) {
		if (path.charCodeAt(i) !== 0x2f /* '/' */) continue;
		const tail = path.slice(i + 1);
		const head = path.slice(0, i);
		if (enc.encode(tail).length > NAME_FIELD) continue;
		if (enc.encode(head).length > PREFIX_FIELD) continue;
		return { prefix: head, name: tail };
	}
	return null;
}

function writeOctal(block: Uint8Array, offset: number, len: number, value: number): void {
	// USTAR octal field: NUL-padded ASCII octal, NUL or space terminator
	// in the last byte. We use NUL — same as GNU tar default.
	const oct = value.toString(8).padStart(len - 1, '0');
	for (let i = 0; i < len - 1; i++) {
		block[offset + i] = oct.charCodeAt(i);
	}
	block[offset + len - 1] = 0;
}
