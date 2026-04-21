// Runtime validation against the JSON Schema bundle shipped from Rust
// (sdk-schemas/akua.json). Every SDK method that parses CLI stdout runs
// the matching validator before handing back a value — contract drift
// between the Rust source and the SDK's compile-time types surfaces as
// a typed `AkuaContractError` at the parse boundary, not later as a
// `NaN` or `undefined.field` ten call frames deep.
//
// Why ajv: research verdict ranked it as the fastest validator (~14M
// ops/s vs Zod v4's ~2M), and we already emit JSON Schema from Rust —
// no transform step needed. Consumers who prefer Zod can derive via
// json-schema-to-zod; this module is the canonical validator.

import Ajv2020, { type ValidateFunction } from 'ajv/dist/2020.js';

import { AkuaError } from './errors.ts';
import akuaSchema from '../../../sdk-schemas/akua.json' with { type: 'json' };

type SchemaBundle = {
	$id: string;
	$defs: Record<string, object>;
};

const bundle = akuaSchema as unknown as SchemaBundle;

// Draft 2020-12 instance (matches the `$schema` in akua.json). `strict: false`
// because schemars emits `format: "uint32"` etc. that ajv doesn't know natively.
const ajv = new Ajv2020({ strict: false, allErrors: true });

// Register the bundle once. Every per-type validator retrieved below is a
// reference *into* this registered schema, so $refs resolve correctly and
// we don't re-register the $id on every compile.
ajv.addSchema(bundle);

const cache = new Map<string, ValidateFunction>();

/**
 * Fetch (and cache) a validator for a top-level type from the bundle.
 * Keyed by the same name used in Rust (`VersionOutput`, `StructuredError`,
 * `ExitCode`, etc.) — matches what ts-rs emits per-type and what schemars
 * puts in `$defs`.
 */
function compile(name: string): ValidateFunction {
	const cached = cache.get(name);
	if (cached) return cached;

	if (!bundle.$defs?.[name]) {
		throw new Error(
			`No schema named "${name}" in sdk-schemas/akua.json — regenerate via \`task sdk:gen\`?`,
		);
	}
	const validator = ajv.getSchema(`${bundle.$id}#/$defs/${name}`);
	if (!validator) {
		throw new Error(`ajv failed to resolve $ref for "${name}"`);
	}
	cache.set(name, validator);
	return validator;
}

/**
 * Thrown when the CLI emits JSON that doesn't match the compiled
 * contract — "we shipped @akua/sdk against contract v1 but the binary
 * we invoked is emitting a v2 shape." Distinct from `AkuaUserError` &
 * friends (which are the CLI *intentionally* reporting a problem).
 */
export class AkuaContractError extends AkuaError {
	readonly schemaName: string;
	readonly validationErrors: ReadonlyArray<object>;
	readonly raw: unknown;

	constructor(schemaName: string, validationErrors: ReadonlyArray<object>, raw: unknown) {
		super(
			`akua --json output failed schema validation for ${schemaName}: ${JSON.stringify(validationErrors)}`,
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

/**
 * Validate `value` against the named type from the bundle. Returns the
 * same reference typed as `T` on success; throws `AkuaContractError` on
 * drift. Used by every SDK method at the `JSON.parse(stdout)` boundary.
 */
export function validateAs<T>(schemaName: string, value: unknown): T {
	const validate = compile(schemaName);
	if (!validate(value)) {
		throw new AkuaContractError(schemaName, validate.errors ?? [], value);
	}
	return value as T;
}
