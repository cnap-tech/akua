// Standard Schema v1 adapter tests. Verifies the shape matches the spec
// (standard-schema.dev) so any framework accepting a StandardSchemaV1
// can consume @akua/sdk's types — Hono, tRPC, SvelteKit form/remote,
// react-hook-form, etc. — without wrapping in Zod or Valibot.

import { test } from 'node:test';
import { strict as assert } from 'node:assert';

import { standardSchemaFor } from './validate.ts';
import type { VersionOutput } from '../../../sdk-types/VersionOutput.ts';

test('exposes the StandardSchemaV1 props block', () => {
	const schema = standardSchemaFor<VersionOutput>('VersionOutput');
	assert.equal(schema['~standard'].version, 1);
	assert.equal(schema['~standard'].vendor, 'akua');
	assert.equal(typeof schema['~standard'].validate, 'function');
});

test('validate returns { value } on success', () => {
	const schema = standardSchemaFor<VersionOutput>('VersionOutput');
	const result = schema['~standard'].validate({ version: '0.1.0' });
	assert.ok('value' in result);
	assert.equal((result as { value: VersionOutput }).value.version, '0.1.0');
});

test('validate returns { issues } on failure, with messages + paths', () => {
	const schema = standardSchemaFor<VersionOutput>('VersionOutput');
	const result = schema['~standard'].validate({ version: 42 });
	assert.ok('issues' in result);
	const { issues } = result as { issues: ReadonlyArray<{ message: string; path?: ReadonlyArray<unknown> }> };
	assert.ok(issues.length > 0);
	assert.equal(typeof issues[0]!.message, 'string');
	// Standard Schema path is an array; ajv's "/version" becomes ["version"].
	assert.deepEqual(issues[0]!.path, ['version']);
});

test('missing required field surfaces as a top-level issue', () => {
	const schema = standardSchemaFor<VersionOutput>('VersionOutput');
	const result = schema['~standard'].validate({});
	assert.ok('issues' in result);
});

test('numeric array indices in path are numbers, not strings', () => {
	// Contrived: validate against StructuredError.next_actions — index path
	// segments must be numbers per StandardSchemaV1.PathSegment spec.
	const schema = standardSchemaFor('StructuredError');
	const result = schema['~standard'].validate({
		code: 'E_X',
		message: 'y',
		next_actions: [42], // wrong: array of strings expected
	});
	assert.ok('issues' in result);
	const { issues } = result as { issues: ReadonlyArray<{ path?: ReadonlyArray<unknown> }> };
	const offending = issues.find((i) => i.path?.some((p) => typeof p === 'number'));
	assert.ok(offending, 'expected an issue with a numeric path segment');
});
