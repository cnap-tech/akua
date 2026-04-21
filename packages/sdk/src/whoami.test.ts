import { test } from 'node:test';
import { strict as assert } from 'node:assert';

import { AkuaContractError, validateAs } from './validate.ts';
import type { WhoamiOutput } from '../../../sdk-types/WhoamiOutput.ts';

function expectContractError(fn: () => unknown) {
	assert.throws(fn, (err: unknown) => err instanceof AkuaContractError);
}

test('validateAs accepts a claude-code-detected whoami payload', () => {
	const w = validateAs<WhoamiOutput>('WhoamiOutput', {
		agent_context: {
			detected: true,
			source: 'claude_code',
			name: '1',
			disabled_via_env: false,
		},
		version: '0.1.0',
	});
	assert.equal(w.agent_context.detected, true);
	assert.equal(w.agent_context.source, 'claude_code');
	assert.equal(w.version, '0.1.0');
});

test('validateAs accepts a not-detected whoami (source/name omitted)', () => {
	const w = validateAs<WhoamiOutput>('WhoamiOutput', {
		agent_context: { detected: false, disabled_via_env: false },
		version: '0.1.0',
	});
	assert.equal(w.agent_context.detected, false);
});

test('validateAs rejects an unknown AgentSource value', () => {
	expectContractError(() =>
		validateAs<WhoamiOutput>('WhoamiOutput', {
			agent_context: {
				detected: true,
				source: 'copilot_cli',
				name: '1',
				disabled_via_env: false,
			},
			version: '0.1.0',
		}),
	);
});

test('validateAs rejects missing version field', () => {
	expectContractError(() =>
		validateAs<WhoamiOutput>('WhoamiOutput', {
			agent_context: { detected: false, disabled_via_env: false },
		}),
	);
});
