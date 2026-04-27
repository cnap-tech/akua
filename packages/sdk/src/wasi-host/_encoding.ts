// Runtime-portable byte ↔ base64 helpers. Both engine handlers
// pass tar.gz blobs to their wasm engine as base64 strings (mirrors
// `crates/{helm,kustomize}-engine-wasm` Rust shim shape).

/**
 * Encode `bytes` as a base64 string. Chunked to keep `String.from
 * CharCode(...big)` from blowing the JS engine's argument stack.
 *
 * Prefers `globalThis.btoa` (Node 24 / Bun / browsers) for runtime
 * portability; falls back to `Buffer` on older Node where `btoa`
 * was Browser-only.
 */
export function bytesToBase64(bytes: Uint8Array): string {
	const binary: string[] = [];
	for (let i = 0; i < bytes.length; i += 0x8000) {
		binary.push(String.fromCharCode(...bytes.subarray(i, i + 0x8000)));
	}
	if (typeof btoa === 'function') {
		return btoa(binary.join(''));
	}
	return Buffer.from(binary.join(''), 'binary').toString('base64');
}
