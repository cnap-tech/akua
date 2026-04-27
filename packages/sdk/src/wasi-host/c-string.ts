// C-string helpers for the wasi-host plugin bridge. The worker
// imports `env.kcl_plugin_invoke_json_wasm(method_ptr, args_ptr,
// kwargs_ptr) -> i32` — three pointers to NUL-terminated UTF-8
// strings in worker linear memory. The host reads them, dispatches,
// allocates a response buffer in worker memory via the worker's
// exported `akua_bridge_alloc`, copies the response bytes + NUL,
// returns the pointer. Mirrors the Rust host shape in
// `crates/akua-cli/src/render_worker.rs`.
//
// IMPORTANT: never hold a `Uint8Array` view across guest calls — the
// engine may grow its `WebAssembly.Memory`, replacing
// `memory.buffer`, leaving the view dangling. Always re-take a fresh
// view from `memory.buffer` immediately before the read/write.

const decoder = new TextDecoder();
const encoder = new TextEncoder();

/**
 * Read a NUL-terminated UTF-8 string from worker memory at `ptr`.
 * Returns `null` if `ptr <= 0` or the read runs off the end without
 * finding a NUL — KCL's bridge convention treats both as "argument
 * absent", per `kcl_plugin::c_str_or_default`.
 */
export function readCString(memory: WebAssembly.Memory, ptr: number): string | null {
	if (ptr <= 0) return null;
	const view = new Uint8Array(memory.buffer);
	const start = ptr >>> 0;
	let end = start;
	while (end < view.length && view[end] !== 0) {
		end += 1;
	}
	if (end >= view.length) return null;
	return decoder.decode(view.subarray(start, end));
}

/**
 * Read a C-string with a fallback for the absent / empty case.
 * Mirrors the Rust host's `read_c_str_or_empty` — KCL serializes
 * empty `args` / `kwargs` as `[]` / `{}`, but skipped fields come
 * through as null pointers.
 */
export function readCStringOr(
	memory: WebAssembly.Memory,
	ptr: number,
	fallback: string,
): string {
	const v = readCString(memory, ptr);
	return v === null || v === '' ? fallback : v;
}

/**
 * Allocate `bytes.length + 1` bytes in worker memory via the worker's
 * exported `akua_bridge_alloc`, copy `bytes` plus a NUL terminator,
 * and return the guest pointer. Used to hand the bridge response back
 * to KCL, which reads it as a C-string.
 *
 * Re-takes the memory view AFTER the alloc call: the alloc may grow
 * worker linear memory, invalidating any prior view.
 */
export function writeCStringTo(
	memory: WebAssembly.Memory,
	alloc: (size: number) => number,
	text: string,
): number {
	const bytes = encoder.encode(text);
	const total = bytes.length + 1;
	const dest = alloc(total);
	if (dest <= 0) {
		throw new Error(`akua_bridge_alloc returned ${dest} for ${total} bytes`);
	}
	const view = new Uint8Array(memory.buffer);
	view.set(bytes, dest);
	view[dest + bytes.length] = 0;
	return dest;
}
