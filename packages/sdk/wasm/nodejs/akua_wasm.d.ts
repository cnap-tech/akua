/* tslint:disable */
/* eslint-disable */

/**
 * Evaluate a Package.k source buffer against an inputs JSON value
 * and return the rendered YAML.
 *
 * * `package_filename` is used for diagnostic rendering only; no
 *   filesystem is touched (there isn't one).
 * * `source` is the Package.k KCL text.
 * * `inputs_json` is an optional JSON string to inject as KCL's
 *   `option("input")`. Pass `null` or an empty string for no
 *   inputs.
 *
 * Returns the rendered top-level YAML (same shape the CLI's
 * sandbox path returns). Errors surface as JS exceptions carrying
 * the KCL diagnostic text.
 */
export function render(package_filename: string, source: string, inputs_json?: string | null): string;

/**
 * Version tag — cheap sanity check for JS consumers that the
 * bundle they loaded matches what they expect.
 */
export function version(): string;
