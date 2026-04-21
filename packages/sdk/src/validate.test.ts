// Tests for runtime JSON Schema validation. Proves the parse-boundary
// guard actually catches contract drift — if akua emits JSON that
// doesn't match the shape @akua/sdk compiled against, we throw a typed
// error instead of silently returning a wrong-shaped value.

import { test } from 'node:test';
import { strict as assert } from 'node:assert';

import { AkuaContractError, validateAs } from './validate.ts';
import type { VersionOutput } from '../../../sdk-types/VersionOutput.ts';

test('validateAs accepts well-shaped VersionOutput', () => {
	const v = validateAs<VersionOutput>('VersionOutput', { version: '0.1.0' });
	assert.equal(v.version, '0.1.0');
});

test('validateAs rejects missing required field', () => {
	assert.throws(
		() => validateAs<VersionOutput>('VersionOutput', {}),
		(err: unknown) => err instanceof AkuaContractError && err.schemaName === 'VersionOutput',
	);
});

test('validateAs rejects wrong-typed field', () => {
	assert.throws(
		() => validateAs<VersionOutput>('VersionOutput', { version: 42 }),
		(err: unknown) => {
			if (!(err instanceof AkuaContractError)) return false;
			return err.validationErrors.length > 0;
		},
	);
});

test('AkuaContractError carries the invalid payload for debugging', () => {
	try {
		validateAs<VersionOutput>('VersionOutput', { wrong: 'field' });
		assert.fail('expected throw');
	} catch (err) {
		assert.ok(err instanceof AkuaContractError);
		assert.deepEqual(err.raw, { wrong: 'field' });
		assert.equal(err.structured?.code, 'E_CONTRACT_VIOLATION');
	}
});

test('validateAs throws for unknown schema names', () => {
	assert.throws(
		() => validateAs('NonexistentType', {}),
		/No schema named "NonexistentType"/,
	);
});
