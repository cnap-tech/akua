import { test } from 'node:test';
import { strict as assert } from 'node:assert';

import { standardSchemaFor } from './validate.ts';
import type { VersionOutput } from './types/VersionOutput.ts';

type IssueResult = { issues: ReadonlyArray<{ message: string; path?: ReadonlyArray<unknown> }> };

function expectIssues(
	schema: ReturnType<typeof standardSchemaFor>,
	value: unknown,
): IssueResult {
	const result = schema['~standard'].validate(value);
	assert.ok('issues' in result, 'expected validation to fail');
	return result as IssueResult;
}

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
	const { issues } = expectIssues(schema, { version: 42 });
	assert.ok(issues.length > 0);
	assert.equal(typeof issues[0]!.message, 'string');
	assert.deepEqual(issues[0]!.path, ['version']);
});

test('missing required field surfaces as a top-level issue', () => {
	const schema = standardSchemaFor<VersionOutput>('VersionOutput');
	expectIssues(schema, {});
});

test('numeric array indices in path are numbers, not strings', () => {
	const schema = standardSchemaFor('StructuredError');
	const { issues } = expectIssues(schema, {
		code: 'E_X',
		message: 'y',
		next_actions: [42],
	});
	const offending = issues.find((i) => i.path?.some((p) => typeof p === 'number'));
	assert.ok(offending, 'expected an issue with a numeric path segment');
});
