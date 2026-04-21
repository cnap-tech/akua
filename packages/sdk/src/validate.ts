// Runtime validation against the JSON Schema bundle generated from Rust.
// Every SDK method that parses CLI stdout runs the matching validator
// before returning, so contract drift throws at the parse boundary
// rather than surfacing later as undefined.field access.

import Ajv2020, { type ErrorObject, type ValidateFunction } from 'ajv/dist/2020.js';
import type { StandardSchemaV1 } from '@standard-schema/spec';

import { AkuaError } from './errors.ts';
import akuaSchema from '../../../sdk-schemas/akua.json' with { type: 'json' };

type SchemaBundle = {
	$id: string;
	$defs: Record<string, object>;
};

const bundle = akuaSchema as unknown as SchemaBundle;

// The runtime-valid set of schema names, derived from the bundle.
// Typos on `validateAs('VersionOuput', …)` now fail `tsc` instead of
// throwing at runtime.
export type SchemaName = keyof typeof akuaSchema.$defs & string;

// schemars emits `format: "uint32"` etc. — the integer bounds are already
// captured structurally, so we register them as no-op formats to silence
// the "unknown format" warnings without losing any checking power.
const ajv = new Ajv2020({ strict: false, allErrors: true });
for (const f of ['uint8', 'uint16', 'uint32', 'uint64', 'int8', 'int16', 'int32', 'int64']) {
	ajv.addFormat(f, true);
}
// Register the bundle once so per-type `$ref` lookups resolve across types.
ajv.addSchema(bundle);

const cache = new Map<string, ValidateFunction>();

function compile(name: SchemaName): ValidateFunction {
	const cached = cache.get(name);
	if (cached) return cached;
	const validator = ajv.getSchema(`${bundle.$id}#/$defs/${name}`);
	if (!validator) {
		throw new Error(
			`No schema named "${name}" in sdk-schemas/akua.json — regenerate via \`task sdk:gen\`?`,
		);
	}
	cache.set(name, validator);
	return validator;
}

export class AkuaContractError extends AkuaError {
	readonly schemaName: SchemaName;
	readonly validationErrors: ReadonlyArray<object>;
	readonly raw: unknown;

	constructor(
		schemaName: SchemaName,
		validationErrors: ReadonlyArray<object>,
		raw: unknown,
	) {
		super(
			`akua --json output failed ${schemaName} schema validation (${validationErrors.length} issue(s))`,
			{
				structured: {
					level: 'error',
					code: 'E_CONTRACT_VIOLATION',
					message: `${schemaName} shape mismatch`,
				},
			},
		);
		this.name = 'AkuaContractError';
		this.schemaName = schemaName;
		this.validationErrors = validationErrors;
		this.raw = raw;
	}
}

export function validateAs<T>(schemaName: SchemaName, value: unknown): T {
	const validate = compile(schemaName);
	if (!validate(value)) {
		throw new AkuaContractError(schemaName, validate.errors ?? [], value);
	}
	return value as T;
}

// ajv's `instancePath` is a JSON Pointer (`/outputs/0/name`); Standard
// Schema wants an array of `PropertyKey`s, with numeric segments as
// numbers per its `PathSegment` spec.
function ajvErrorToIssue(err: ErrorObject): StandardSchemaV1.Issue {
	const segments = (err.instancePath || '')
		.split('/')
		.filter(Boolean)
		.map((seg) => (seg.match(/^\d+$/) ? Number(seg) : seg));
	const msg = err.message ?? 'invalid value';
	return segments.length > 0 ? { message: msg, path: segments } : { message: msg };
}

export function standardSchemaFor<T>(schemaName: SchemaName): StandardSchemaV1<unknown, T> {
	const validate = compile(schemaName);
	return {
		'~standard': {
			version: 1,
			vendor: 'akua',
			validate(value) {
				if (validate(value)) return { value: value as T };
				return { issues: (validate.errors ?? []).map(ajvErrorToIssue) };
			},
		},
	};
}
