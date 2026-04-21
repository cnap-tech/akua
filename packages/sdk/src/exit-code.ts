// Numeric exit code (from Node's `child.exitCode`) ↔ the kebab-case
// name Rust ships. Index in BY_NUMBER = integer value in Rust (matches
// akua-core::ExitCode::from_code).

import type { ExitCode } from '../../../sdk-types/ExitCode.ts';

const BY_NUMBER: readonly ExitCode[] = [
	'success',
	'user-error',
	'system-error',
	'policy-deny',
	'rate-limited',
	'needs-approval',
	'timeout',
];

/** Map `child.exitCode` → the typed name, or `undefined` for unknown codes. */
export function exitCodeFromNumber(n: number | null): ExitCode | undefined {
	if (n == null || n < 0 || n >= BY_NUMBER.length) return undefined;
	return BY_NUMBER[n];
}
