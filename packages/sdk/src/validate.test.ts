import { test } from 'node:test';
import { strict as assert } from 'node:assert';

import { AkuaContractError, validateAs } from './validate.ts';
import type { VersionOutput } from './types/VersionOutput.ts';

function expectContractError(fn: () => unknown, schemaName?: string) {
	try {
		fn();
		assert.fail('expected AkuaContractError');
	} catch (err) {
		assert.ok(err instanceof AkuaContractError, `got ${err?.constructor?.name}`);
		if (schemaName) assert.equal(err.schemaName, schemaName);
		return err;
	}
}

test('validateAs accepts well-shaped VersionOutput', () => {
	const v = validateAs<VersionOutput>('VersionOutput', { version: '0.1.0' });
	assert.equal(v.version, '0.1.0');
});

test('validateAs rejects missing required field', () => {
	expectContractError(() => validateAs<VersionOutput>('VersionOutput', {}), 'VersionOutput');
});

test('validateAs rejects wrong-typed field', () => {
	const err = expectContractError(
		() => validateAs<VersionOutput>('VersionOutput', { version: 42 }),
		'VersionOutput',
	);
	assert.ok(err.validationErrors.length > 0);
});

test('AkuaContractError carries the invalid payload for debugging', () => {
	const err = expectContractError(
		() => validateAs<VersionOutput>('VersionOutput', { wrong: 'field' }),
	);
	assert.deepEqual(err.raw, { wrong: 'field' });
	assert.equal(err.structured?.code, 'E_CONTRACT_VIOLATION');
});
