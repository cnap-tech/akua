/**
 * Runtime-specific WASM loader. The build pipeline places the nodejs
 * artifact at `../wasm/nodejs/` and the bundler (browser) artifact at
 * `../wasm/bundler/`. The Node entry point imports one, the browser
 * entry point imports the other — see index.node.ts / index.browser.ts.
 *
 * Each wasm-pack target exposes the same function names with identical
 * semantics, so downstream code only ever sees the `WasmApi` interface.
 */

import { WasmInitError } from './errors.js';

export interface WasmApi {
  extractUserInputFields(schema: unknown): unknown;
  applyInputTransforms(fields: unknown, inputs: unknown): unknown;
  validateValuesSchema(schema: unknown): string | null | undefined;
  mergeSourceValues(sources: unknown): unknown;
  mergeValuesSchemas(sources: unknown): unknown;
  buildUmbrellaChart(name: string, version: string, sources: unknown): unknown;
  buildMetadata(sources: unknown, fields: unknown, buildTime: string): unknown;
}

let api: WasmApi | null = null;

/**
 * Install a concrete WASM implementation. Called once by the env-
 * specific entry point (Node or browser). Subsequent calls are no-ops —
 * the first loader to win is the one we use.
 */
export function setWasmApi(loaded: WasmApi): void {
  if (!api) api = loaded;
}

/** Fetch the previously-installed WASM API. Throws if init didn't run. */
export function wasm(): WasmApi {
  if (!api) {
    throw new WasmInitError(
      '@akua/sdk: WASM not initialised. Call `await init()` before any other SDK function.',
    );
  }
  return api;
}
