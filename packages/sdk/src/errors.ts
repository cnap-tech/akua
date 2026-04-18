/**
 * Error class hierarchy for the SDK. Everything SDK-thrown extends
 * [`AkuaError`] so consumers can `instanceof` once when mapping to
 * domain errors (Temporal `ApplicationFailure`, HTTP 4xx/5xx, etc.).
 *
 * Transport errors from the global `fetch` / `DOMException` pass
 * through unwrapped — we don't catch-and-rethrow the network stack.
 */

export class AkuaError extends Error {
  constructor(message: string, readonly cause_?: unknown) {
    super(message);
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
