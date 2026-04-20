/**
 * Error class hierarchy for the SDK. Everything SDK-thrown extends
 * [`AkuaError`] so consumers can `instanceof` once when mapping to
 * domain errors (workflow-engine failures, HTTP 4xx/5xx, etc.).
 *
 * Transport errors from the global `fetch` / `DOMException` pass
 * through unwrapped — we don't catch-and-rethrow the network stack.
 */

export class AkuaError extends Error {
  constructor(message: string, cause?: unknown) {
    super(message, cause !== undefined ? { cause } : undefined);
    this.name = 'AkuaError';
  }
}

/** Thrown from SDK calls made before [`init`] resolved. */
export class WasmInitError extends AkuaError {
  constructor(message: string) {
    super(message);
    this.name = 'WasmInitError';
  }
}
