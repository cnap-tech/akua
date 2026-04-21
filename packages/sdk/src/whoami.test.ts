// Verifies that a realistic `akua whoami --json` payload round-trips
// through the SDK validator cleanly. Exercises the nested-type case
// (WhoamiOutput → AgentContext → AgentSource) that version didn't.

import { test } from 'node:test';
import { strict as assert } from 'node:assert';

import { AkuaContractError, validateAs } from './validate.ts';
import type { WhoamiOutput } from '../../../sdk-types/WhoamiOutput.ts';

test('validateAs accepts a claude-code-detected whoami payload', () => {
	const payload = {
		agent_context: {
			detected: true,
			source: 'claude_code',
			name: '1',
			disabled_via_env: false,
		},
		version: '0.1.0',
	};
	const w = validateAs<WhoamiOutput>('WhoamiOutput', payload);
	assert.equal(w.agent_context.detected, true);
	assert.equal(w.agent_context.source, 'claude_code');
	assert.equal(w.version, '0.1.0');
});

test('validateAs accepts a not-detected whoami (source/name omitted)', () => {
	const payload = {
		agent_context: {
			detected: false,
			disabled_via_env: false,
		},
		version: '0.1.0',
	};
	const w = validateAs<WhoamiOutput>('WhoamiOutput', payload);
	assert.equal(w.agent_context.detected, false);
});

test('validateAs rejects an unknown AgentSource value', () => {
	const payload = {
		agent_context: {
			detected: true,
			source: 'copilot_cli', // not in the enum
			name: '1',
			disabled_via_env: false,
		},
		version: '0.1.0',
	};
	assert.throws(
		() => validateAs<WhoamiOutput>('WhoamiOutput', payload),
		(err: unknown) => err instanceof AkuaContractError,
	);
});

test('validateAs rejects missing version field', () => {
	const payload = {
		agent_context: { detected: false, disabled_via_env: false },
	};
	assert.throws(
		() => validateAs<WhoamiOutput>('WhoamiOutput', payload),
		(err: unknown) => err instanceof AkuaContractError,
	);
});
