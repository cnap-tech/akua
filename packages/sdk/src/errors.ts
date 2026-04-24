// Error hierarchy for `@akua/sdk`. Consumers branch either via
// `instanceof AkuaUserError` (stack name preserved per subclass) or
// by reading `err.structured.code` — the stable `E_*` identifier
// cli-contract §1.2 guarantees.

import type { ExitCode } from './types/ExitCode.ts';
import type { StructuredError } from './types/StructuredError.ts';

import { exitCodeFromNumber } from './exit-code.ts';

export interface AkuaErrorInit {
	structured?: StructuredError;
	exitCode?: ExitCode;
	rawExitCode?: number | null;
	cause?: unknown;
}

export class AkuaError extends Error {
	readonly structured: StructuredError | undefined;
	readonly exitCode: ExitCode | undefined;
	readonly rawExitCode: number | null;

	constructor(message: string, init: AkuaErrorInit = {}) {
		super(message, { cause: init.cause });
		this.name = 'AkuaError';
		this.structured = init.structured;
		this.exitCode = init.exitCode;
		this.rawExitCode = init.rawExitCode ?? null;
	}
}

export class AkuaTransportError extends AkuaError {
	constructor(message: string, init: AkuaErrorInit = {}) {
		super(message, init);
		this.name = 'AkuaTransportError';
	}
}

// Subclasses are distinct classes (not tagged instances) so `instanceof`
// works and Node's stack traces carry the real class name. The explicit
// constructor type is required for JSR slow-type checking — otherwise
// JSR can't emit `.d.ts` for Node consumers.
export type AkuaErrorSubclass = new (
	message: string,
	init?: Omit<AkuaErrorInit, 'exitCode'>,
) => AkuaError;

function defineExitError(
	name: string,
	exitCode: Exclude<ExitCode, 'success'>,
): AkuaErrorSubclass {
	class Cls extends AkuaError {
		constructor(message: string, init: Omit<AkuaErrorInit, 'exitCode'> = {}) {
			super(message, { ...init, exitCode });
			this.name = name;
		}
	}
	Object.defineProperty(Cls, 'name', { value: name });
	return Cls;
}

export const AkuaUserError: AkuaErrorSubclass = defineExitError('AkuaUserError', 'user-error');
export const AkuaSystemError: AkuaErrorSubclass = defineExitError('AkuaSystemError', 'system-error');
export const AkuaPolicyDenyError: AkuaErrorSubclass = defineExitError(
	'AkuaPolicyDenyError',
	'policy-deny',
);
export const AkuaRateLimitedError: AkuaErrorSubclass = defineExitError(
	'AkuaRateLimitedError',
	'rate-limited',
);
export const AkuaNeedsApprovalError: AkuaErrorSubclass = defineExitError(
	'AkuaNeedsApprovalError',
	'needs-approval',
);
export const AkuaTimeoutError: AkuaErrorSubclass = defineExitError('AkuaTimeoutError', 'timeout');

const BY_EXIT_CODE: Record<Exclude<ExitCode, 'success'>, AkuaErrorSubclass> = {
	'user-error': AkuaUserError,
	'system-error': AkuaSystemError,
	'policy-deny': AkuaPolicyDenyError,
	'rate-limited': AkuaRateLimitedError,
	'needs-approval': AkuaNeedsApprovalError,
	timeout: AkuaTimeoutError,
};

// Cap the stderr scan — a runaway CLI could emit millions of log lines
// before exit, and we only need to find one JSONL record.
const MAX_STDERR_LINES_SCANNED = 200;

export function parseStructuredError(stderr: string | undefined): StructuredError | undefined {
	if (!stderr) return undefined;
	// StructuredError is normally the *last* thing before process exit — scan
	// from the end.
	const lines = stderr.split('\n');
	const start = Math.max(0, lines.length - MAX_STDERR_LINES_SCANNED);
	for (let i = lines.length - 1; i >= start; i--) {
		const trimmed = lines[i]!.trim();
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
	const init = { structured, rawExitCode, cause };

	if (exitCode && exitCode !== 'success') {
		return new BY_EXIT_CODE[exitCode](message, init);
	}
	// Unknown numeric code, or a success exit paired with an error payload
	// (contract violation — still surface rather than drop).
	return new AkuaError(message, init);
}
