// End-to-end: spawn the real `akua` binary, run `version()` + `whoami()`,
// confirm the full pipe (spawn → --json → ajv validate → typed return)
// closes without error.
//
// Gated on AKUA_E2E=1 because it requires a built binary at
// ../../target/debug/akua — `task sdk:e2e` builds it first and sets the
// env var. Default `task sdk:test` skips, so the unit-test suite stays
// fast and hermetic.

import { test } from 'node:test';
import { strict as assert } from 'node:assert';

import { Akua } from './mod.ts';
import { BINARY, E2E_ENABLED, assertBinaryBuilt } from './test-utils.ts';

test('Akua.version() returns the binary version', { skip: !E2E_ENABLED }, async () => {
	assertBinaryBuilt();
	const akua = new Akua({ binary: BINARY });
	const v = await akua.version();
	assert.match(v.version, /^\d+\.\d+\.\d+/, `got: ${JSON.stringify(v)}`);
});

test(
	'Akua.whoami() returns a typed WhoamiOutput with agent_context',
	{ skip: !E2E_ENABLED },
	async () => {
		const akua = new Akua({ binary: BINARY });
		const w = await akua.whoami();
		assert.equal(typeof w.agent_context.detected, 'boolean');
		assert.equal(typeof w.version, 'string');
	},
);
