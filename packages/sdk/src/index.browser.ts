/**
 * Browser / bundler entry point. The `wasm-pack --target bundler`
 * artifact lazy-instantiates its `.wasm` file on first call, so `init()`
 * here kicks the bundler-handled import and waits for instantiation
 * before returning.
 *
 * The bundler (Vite / webpack / esbuild / Bun) handles the `.wasm`
 * asset — ship it, inline it, or fetch it at runtime — per its own
 * config. We don't prescribe.
 */

import { setWasmApi, type WasmApi } from './wasm.js';

let loaded = false;

export async function init(): Promise<void> {
  if (loaded) return;
  const mod = (await import('../wasm/bundler/akua_wasm.js')) as unknown as WasmApi & {
    default?: () => Promise<unknown>;
  };
  // wasm-pack bundler target sometimes exposes a default() for explicit
  // init; call it if present, fall through otherwise.
  if (typeof mod.default === 'function') {
    await mod.default();
  }
  setWasmApi(mod);
  loaded = true;
}

export * from './index.js';
