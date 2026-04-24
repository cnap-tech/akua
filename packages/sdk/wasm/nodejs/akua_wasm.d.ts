/* tslint:disable */
/* eslint-disable */

/**
 * Run the three structural gates. Every source is optional — the
 * CLI verb surfaces missing-file errors at the file-reading layer;
 * this pure primitive only checks the source buffers it's given.
 */
export function check(manifest?: string | null, lock?: string | null, package_filename?: string | null, package_source?: string | null): string;

/**
 * Diff two `{ "path": "sha256-hex" }` maps passed as JSON strings.
 * Returns the `DirDiff` JSON shape `akua diff --json` emits.
 */
export function diff(before_json: string, after_json: string): string;

/**
 * Format a KCL source buffer. `check_mode=true` is read-only and
 * reports `changed` per file; `check_mode=false` returns the
 * formatted text in the `formatted` field (JS writes back to disk).
 * JSON shape: `{ "files": [{ "path": "<filename>", "changed": bool }], "formatted": "..." }`.
 */
export function fmt(filename: string, source: string, check_mode: boolean): string;

/**
 * Introspect a Package.k source buffer — list its `option()` call
 * sites for SDK consumers that want to drive inputs programmatically.
 * JSON shape matches `akua inspect --json --package …` (kind=package).
 */
export function inspect_package(filename: string, source: string): string;

/**
 * Parse a Package.k source buffer and return lint issues.
 * JSON shape: `{ "status": "ok"|"fail", "issues": [...] }` —
 * matches `akua lint --json`.
 */
export function lint(filename: string, source: string): string;

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
 * Walk manifest + optional lock and produce the tree output.
 */
export function tree(manifest: string, lock?: string | null): string;

/**
 * Version tag — cheap sanity check for JS consumers that the
 * bundle they loaded matches what they expect.
 */
export function version(): string;
