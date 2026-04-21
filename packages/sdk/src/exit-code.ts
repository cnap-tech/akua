// Numeric exit code (what Node's `child.exitCode` gives us) ↔ the kebab-case
// name Rust ships. Kept in lockstep with akua-core's `ExitCode::name` /
// `ExitCode::from_code`.

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

/** Kebab-case → numeric, for completeness (inverse of [`exitCodeFromNumber`]). */
export function exitCodeToNumber(name: ExitCode): number {
	return BY_NUMBER.indexOf(name);
}

/** `true` when retrying with the same inputs might succeed — matches the Rust predicate. */
export function isRetriable(code: ExitCode): boolean {
	return code === 'rate-limited' || code === 'timeout';
}
