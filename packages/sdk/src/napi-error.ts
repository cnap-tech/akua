// Bridge napi-thrown errors → SDK typed error subclasses.
//
// `crates/akua-napi/src/lib.rs::into_napi` serializes the verb's
// `StructuredError` + numeric `exit_code` into the thrown napi
// `Error.message`. We parse that JSON and route to the same
// `AkuaUserError` / `AkuaSystemError` / etc. subclasses
// `classifyCliError` produces — so consumers get identical typed
// errors regardless of the transport (shell-out historically vs
// napi today).

import {
	AkuaError,
	classifyByExitCode,
	isStructuredErrorShape,
} from './errors.ts';
import type { StructuredError } from './types/StructuredError.ts';

interface NapiErrorEnvelope extends StructuredError {
	exit_code?: number;
}

/**
 * Parse a thrown error from the napi addon and rebuild it as the
 * matching SDK `AkuaError` subclass. Returns `undefined` when the
 * thrown error isn't shaped like a napi structured error (e.g. it
 * was thrown by JS code outside the addon, or napi failed to
 * serialize the structured error at all).
 */
export function parseNapiError(err: unknown): AkuaError | undefined {
	if (!(err instanceof Error)) return undefined;
	const envelope = decodeEnvelope(err.message);
	if (!envelope) return undefined;
	const exitCode = envelope.exit_code;
	const cls =
		typeof exitCode === 'number' ? classifyByExitCode(exitCode) : undefined;
	const init = {
		structured: envelope,
		rawExitCode: exitCode ?? null,
		cause: err,
	};
	if (cls) return new cls(envelope.message, init);
	return new AkuaError(envelope.message, init);
}

function decodeEnvelope(raw: string): NapiErrorEnvelope | undefined {
	const trimmed = raw.trim();
	if (!trimmed.startsWith('{')) return undefined;
	let parsed: unknown;
	try {
		parsed = JSON.parse(trimmed);
	} catch {
		return undefined;
	}
	if (!isStructuredErrorShape(parsed)) return undefined;
	return parsed as NapiErrorEnvelope;
}
