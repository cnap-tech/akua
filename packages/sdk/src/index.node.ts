/**
 * Node entry point. Loads the `wasm-pack --target nodejs` artifact,
 * which does sync initialization via `require` / dynamic import — no
 * `fetch()` or `WebAssembly.instantiate` handshake needed, so `init()`
 * here is effectively free. We keep the async signature for API
 * uniformity with the browser entry.
 */

import { setWasmApi, type WasmApi } from './wasm.js';

let loaded = false;

export async function init(): Promise<void> {
  if (loaded) return;
  const mod = (await import('../wasm/nodejs/akua_wasm.js')) as unknown as WasmApi;
  setWasmApi(mod);
  loaded = true;
}

export * from './index.js';
