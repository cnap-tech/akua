// Error hierarchy for `@akua/sdk`. Every failure the SDK surfaces is an
// `AkuaError` or subclass — consumers do `instanceof AkuaUserError` for
// branch-logic, or read `err.structured.code` (the stable `E_*` identifier
// from cli-contract §1.2) for exhaustive matching.
//
// Design notes:
// - The base class carries the parsed `StructuredError` (when the CLI
//   emitted one on stderr) + the typed `ExitCode`. Both are optional on
//   the base, but every subclass guarantees one of them.
// - Subclasses are keyed 1:1 with the `ExitCode` enum variants (except
//   `Success`, which is not an error). Adding a new exit code is a Rust
//   + TS change in lockstep — the compiler enforces coverage.
// - `AkuaTransportError` is separate: the binary failed to spawn at all
//   (ENOENT, permission denied, etc.). No structured error, no exit code.
// - We preserve the native `Error.cause` chain so stack traces and root
//   causes survive — no re-wrapping that drops the original.

import type { ExitCode } from '../../../sdk-types/ExitCode.ts';
import type { StructuredError } from '../../../sdk-types/StructuredError.ts';

import { exitCodeFromNumber } from './exit-code.ts';

export interface AkuaErrorOptions {
	cause?: unknown;
}

export class AkuaError extends Error {
	readonly structured: StructuredError | undefined;
	readonly exitCode: ExitCode | undefined;
	readonly rawExitCode: number | null;

	constructor(
		message: string,
		init: {
			structured?: StructuredError;
			exitCode?: ExitCode;
			rawExitCode?: number | null;
		} = {},
		options: AkuaErrorOptions = {},
	) {
		super(message, options.cause !== undefined ? { cause: options.cause } : undefined);
		this.name = 'AkuaError';
		this.structured = init.structured;
		this.exitCode = init.exitCode;
		this.rawExitCode = init.rawExitCode ?? null;
	}
}

/** Binary failed to spawn (not found, permission denied, etc.). */
export class AkuaTransportError extends AkuaError {
	constructor(message: string, options: AkuaErrorOptions = {}) {
		super(message, {}, options);
		this.name = 'AkuaTransportError';
	}
}

/** Exit 1 — invalid inputs, bad flags, missing required args. */
export class AkuaUserError extends AkuaError {
	constructor(message: string, init: { structured?: StructuredError; rawExitCode: number }) {
		super(message, { ...init, exitCode: 'user-error' });
		this.name = 'AkuaUserError';
	}
}

/** Exit 2 — unexpected failure (disk, network, bug) not caused by the caller. */
export class AkuaSystemError extends AkuaError {
	constructor(message: string, init: { structured?: StructuredError; rawExitCode: number }) {
		super(message, { ...init, exitCode: 'system-error' });
		this.name = 'AkuaSystemError';
	}
}

/** Exit 3 — policy engine rejected the operation. */
export class AkuaPolicyDenyError extends AkuaError {
	constructor(message: string, init: { structured?: StructuredError; rawExitCode: number }) {
		super(message, { ...init, exitCode: 'policy-deny' });
		this.name = 'AkuaPolicyDenyError';
	}
}

/** Exit 4 — registry / API rate limits. Retry with backoff. */
export class AkuaRateLimitedError extends AkuaError {
	constructor(message: string, init: { structured?: StructuredError; rawExitCode: number }) {
		super(message, { ...init, exitCode: 'rate-limited' });
		this.name = 'AkuaRateLimitedError';
	}
}

/** Exit 5 — allowed but requires human approval before proceeding. */
export class AkuaNeedsApprovalError extends AkuaError {
	constructor(message: string, init: { structured?: StructuredError; rawExitCode: number }) {
		super(message, { ...init, exitCode: 'needs-approval' });
		this.name = 'AkuaNeedsApprovalError';
	}
}

/** Exit 6 — operation did not complete within `--timeout`. */
export class AkuaTimeoutError extends AkuaError {
	constructor(message: string, init: { structured?: StructuredError; rawExitCode: number }) {
		super(message, { ...init, exitCode: 'timeout' });
		this.name = 'AkuaTimeoutError';
	}
}

/**
 * Find the first line in `stderr` that parses as a `StructuredError`.
 * cli-contract §1.2 guarantees these are emitted as JSON-Lines — one
 * record per line, no trailing newline inside the record.
 */
export function parseStructuredError(stderr: string | undefined): StructuredError | undefined {
	if (!stderr) return undefined;
	for (const line of stderr.split('\n')) {
		const trimmed = line.trim();
		if (!trimmed.startsWith('{')) continue;
		try {
			const parsed = JSON.parse(trimmed) as unknown;
			if (
				parsed &&
				typeof parsed === 'object' &&
				'code' in parsed &&
				'message' in parsed &&
				typeof (parsed as { code: unknown }).code === 'string' &&
				typeof (parsed as { message: unknown }).message === 'string'
			) {
				return parsed as StructuredError;
			}
		} catch {
			// Not JSON — keep scanning.
		}
	}
	return undefined;
}

/**
 * Turn a failed `execFile` result into the right subclass. Pulls the
 * structured error off stderr (when present) and uses its `message` as
 * the thrown `Error.message` so `toString()` surfaces the CLI's intent.
 */
export function classifyCliError(input: {
	rawExitCode: number | null;
	stderr: string | undefined;
	cause?: unknown;
}): AkuaError {
	const { rawExitCode, stderr, cause } = input;

	if (rawExitCode == null) {
		return new AkuaTransportError('failed to spawn akua', { cause });
	}

	const exitCode = exitCodeFromNumber(rawExitCode);
	const structured = parseStructuredError(stderr);
	const message = structured?.message ?? `akua exited with code ${rawExitCode}`;
	const init = { structured, rawExitCode };

	switch (exitCode) {
		case 'user-error':
			return new AkuaUserError(message, init);
		case 'system-error':
			return new AkuaSystemError(message, init);
		case 'policy-deny':
			return new AkuaPolicyDenyError(message, init);
		case 'rate-limited':
			return new AkuaRateLimitedError(message, init);
		case 'needs-approval':
			return new AkuaNeedsApprovalError(message, init);
		case 'timeout':
			return new AkuaTimeoutError(message, init);
		case 'success':
		case undefined:
			// Unknown numeric code or 0-with-error (contract violation; still surface it).
			return new AkuaError(message, { structured, rawExitCode }, { cause });
	}
}
