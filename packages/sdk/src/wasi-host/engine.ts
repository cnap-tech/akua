// Generic Node-WASI host for an akua engine .wasm. helm-engine and
// kustomize-engine share the same ABI: a Go-built reactor module
// exporting `_initialize`, `<prefix>_malloc`, `<prefix>_free`,
// `<prefix>_result_len`, and an `<entry>` function with signature
// `(input_ptr, input_len) -> result_ptr`. Mirrors the Rust shape in
// `crates/engine-host-wasm/src/lib.rs::Session`.
//
// One Engine per process per .wasm file: `_initialize` runs the Go
// reactor + package-init chain, which is expensive (~140ms cold)
// and must run exactly once. After init, repeat `entry()` calls are
// cheap (~10ms warm).

import { readFileSync } from 'node:fs';
import { WASI } from 'node:wasi';

export interface EngineSpec {
	/** Path to the engine `.wasm` file. */
	wasmPath: string;
	/** Symbol prefix for allocator exports — `helm`, `kustomize`, … */
	prefix: string;
	/** Export symbol of the entry-point function — `helm_render`, etc. */
	entry: string;
	/** Diagnostic name; becomes argv[0] inside the engine. */
	name: string;
}

export class Engine {
	readonly memory: WebAssembly.Memory;
	readonly malloc: (size: number) => number;
	readonly free: (ptr: number) => void;
	readonly entryFn: (inputPtr: number, inputLen: number) => number;
	readonly resultLen: (ptr: number) => number;

	private constructor(
		memory: WebAssembly.Memory,
		malloc: (size: number) => number,
		free: (ptr: number) => void,
		entryFn: (inputPtr: number, inputLen: number) => number,
		resultLen: (ptr: number) => number,
	) {
		this.memory = memory;
		this.malloc = malloc;
		this.free = free;
		this.entryFn = entryFn;
		this.resultLen = resultLen;
	}

	static async load(spec: EngineSpec): Promise<Engine> {
		const wasi = new WASI({
			version: 'preview1',
			args: [spec.name],
			env: {},
			preopens: {},
		});

		const wasm = readFileSync(spec.wasmPath);
		const module_ = await WebAssembly.compile(wasm);
		const instance = await WebAssembly.instantiate(module_, {
			wasi_snapshot_preview1: wasi.wasiImport,
		});

		// Reactor: `_initialize` runs Go's package-init chain (klog,
		// helm/kustomize SDK setup). Exactly once before any `<entry>`
		// call. node:wasi.initialize() invokes the wasm `_initialize`
		// export and validates the module shape; throws if the module
		// has `_start` instead (programmatic, not reactor).
		wasi.initialize(instance);

		const exports = instance.exports as Record<string, WebAssembly.ExportValue>;
		const requireExport = <T>(name: string): T => {
			const value = exports[name];
			if (value === undefined) {
				throw new Error(
					`${spec.name}: ${spec.wasmPath} missing required export \`${name}\``,
				);
			}
			return value as T;
		};

		return new Engine(
			requireExport<WebAssembly.Memory>('memory'),
			requireExport<(size: number) => number>(`${spec.prefix}_malloc`),
			requireExport<(ptr: number) => void>(`${spec.prefix}_free`),
			requireExport<(inputPtr: number, inputLen: number) => number>(spec.entry),
			requireExport<(ptr: number) => number>(`${spec.prefix}_result_len`),
		);
	}

	/**
	 * Round-trip a JSON byte buffer through the engine. Caller copies
	 * `input` into the engine's linear memory via `<prefix>_malloc`,
	 * invokes `<entry>`, reads the result by length-probing
	 * `<prefix>_result_len`, then `<prefix>_free`s both pointers.
	 *
	 * The result is `.slice()`d into a fresh `Uint8Array` so the
	 * caller can safely use it after subsequent engine calls — guest
	 * memory may be reused / freed once we return.
	 */
	call(input: Uint8Array): Uint8Array {
		const inputPtr = this.malloc(input.length);
		new Uint8Array(this.memory.buffer, inputPtr, input.length).set(input);

		const resultPtr = this.entryFn(inputPtr, input.length);
		const len = this.resultLen(resultPtr);
		// Re-take the memory view: `entry` may have grown memory.
		const out = new Uint8Array(this.memory.buffer, resultPtr, len).slice();

		// Best-effort: guest reuses freed pointers on next alloc, so a
		// dropped free here costs only fragmentation. Mirrors the Rust
		// engine-host-wasm contract.
		try {
			this.free(inputPtr);
		} catch {
			/* drop */
		}
		try {
			this.free(resultPtr);
		} catch {
			/* drop */
		}

		return out;
	}
}
