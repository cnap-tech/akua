// End-to-end: spawn the real `akua` binary, run `version()` + `whoami()`,
// confirm the full pipe (spawn → --json → ajv validate → typed return)
// closes without error.
//
// Gated on AKUA_E2E=1 because it requires a built binary at
// ../../target/debug/akua — `task sdk:e2e` builds it first and sets the
// env var. Default `task sdk:test` skips, so the unit-test suite stays
// fast and hermetic.

import { describe, expect, test } from 'bun:test';

import { Akua } from './mod.ts';
import { BINARY, E2E_ENABLED, assertBinaryBuilt } from './test-utils.ts';

describe.if(E2E_ENABLED)('Akua e2e (shell-out)', () => {
	test('Akua.version() returns the binary version', async () => {
		assertBinaryBuilt();
		const akua = new Akua({ binary: BINARY });
		const v = await akua.version();
		expect(v.version).toMatch(/^\d+\.\d+\.\d+/);
	});

	test('Akua.whoami() returns a typed WhoamiOutput with agent_context', async () => {
		const akua = new Akua({ binary: BINARY });
		const w = await akua.whoami();
		expect(typeof w.agent_context.detected).toBe('boolean');
		expect(typeof w.version).toBe('string');
	});
});
