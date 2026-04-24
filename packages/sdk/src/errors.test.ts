import { test } from 'node:test';
import { strict as assert } from 'node:assert';

import {
	AkuaError,
	AkuaPolicyDenyError,
	AkuaRateLimitedError,
	AkuaTimeoutError,
	AkuaUserError,
	classifyCliError,
	parseStructuredError,
} from './errors.ts';

test('parseStructuredError finds the JSONL record on stderr', () => {
	const stderr = [
		'some preamble text line (ignored)',
		'{"level":"error","code":"E_SCHEMA_INVALID","message":"expected integer, got string","path":"inputs.yaml"}',
		'trailing text (also ignored)',
	].join('\n');

	const parsed = parseStructuredError(stderr);
	assert.equal(parsed?.code, 'E_SCHEMA_INVALID');
	assert.equal(parsed?.message, 'expected integer, got string');
	assert.equal(parsed?.path, 'inputs.yaml');
});

test('parseStructuredError returns undefined when stderr has no JSON', () => {
	assert.equal(parseStructuredError('just some log text'), undefined);
	assert.equal(parseStructuredError(''), undefined);
	assert.equal(parseStructuredError(undefined), undefined);
});

test('classifyCliError maps exit 1 → AkuaUserError', () => {
	const err = classifyCliError({
		rawExitCode: 1,
		stderr: '{"level":"error","code":"E_BAD_INPUT","message":"missing required arg"}',
	});
	assert.ok(err instanceof AkuaUserError);
	assert.equal(err.exitCode, 'user-error');
	assert.equal(err.structured?.code, 'E_BAD_INPUT');
	assert.equal(err.message, 'missing required arg');
});

test('classifyCliError maps exit 3 → AkuaPolicyDenyError', () => {
	const err = classifyCliError({
		rawExitCode: 3,
		stderr: '{"level":"error","code":"E_POLICY_DENY","message":"workload disallowed"}',
	});
	assert.ok(err instanceof AkuaPolicyDenyError);
	assert.equal(err.exitCode, 'policy-deny');
});

test('classifyCliError maps exit 4 → AkuaRateLimitedError', () => {
	const err = classifyCliError({
		rawExitCode: 4,
		stderr: '{"level":"error","code":"E_RATE_LIMITED","message":"slow down"}',
	});
	assert.ok(err instanceof AkuaRateLimitedError);
});

test('classifyCliError maps exit 6 → AkuaTimeoutError', () => {
	const err = classifyCliError({
		rawExitCode: 6,
		stderr: '{"level":"error","code":"E_TIMEOUT","message":"exceeded --timeout"}',
	});
	assert.ok(err instanceof AkuaTimeoutError);
});

test('classifyCliError falls back to generic AkuaError for unknown exit codes', () => {
	const err = classifyCliError({ rawExitCode: 99, stderr: '' });
	assert.ok(err instanceof AkuaError);
	assert.equal(err instanceof AkuaUserError, false);
	assert.equal(err.exitCode, undefined);
	assert.equal(err.rawExitCode, 99);
});

test('classifyCliError without StructuredError falls back to a code-based message', () => {
	const err = classifyCliError({ rawExitCode: 2, stderr: 'just some noise' });
	assert.match(err.message, /exited with code 2/);
	assert.equal(err.structured, undefined);
});

test('AkuaError preserves native Error.cause', () => {
	const cause = new Error('underlying spawn failure');
	const err = classifyCliError({ rawExitCode: null, stderr: undefined, cause });
	assert.equal(err.cause, cause);
});
