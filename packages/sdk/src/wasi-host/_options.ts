// Plugin-call argument extraction shared by helm/kustomize handlers.
// Mirrors `crates/akua-core/src/kcl_plugin::extract_options_arg` —
// every engine plugin receives `args = [<schemaInstance>]` from KCL,
// where the schema instance is a JSON object whose fields are the
// plugin options. Failures throw with the plugin-name prefix so the
// downstream `__kcl_PanicInfo__` envelope reads cleanly.

/**
 * Extract the single options object from `args`. Throws a plugin-
 * prefixed error if `args` is empty or its first entry isn't an
 * object (KCL packages typed schemas like `helm.Template { ... }`).
 */
export function extractOptions(
	args: unknown[],
	pluginName: string,
	schemaName: string,
): Record<string, unknown> {
	if (args.length === 0) {
		throw new Error(`${pluginName}: missing options argument (${schemaName} instance)`);
	}
	const first = args[0];
	if (first === null || typeof first !== 'object' || Array.isArray(first)) {
		throw new Error(`${pluginName}: options argument must be a ${schemaName} instance`);
	}
	return first as Record<string, unknown>;
}

/**
 * Read a required string field from an options object. `errMsg` is
 * appended verbatim after the plugin-name prefix so callers can
 * mirror the Rust crate's error wording (e.g. `options.chart must
 * be a string`).
 */
export function requireStringField(
	obj: Record<string, unknown>,
	key: string,
	pluginName: string,
	errMsg: string,
): string {
	const v = obj[key];
	if (typeof v !== 'string') {
		throw new Error(`${pluginName}: ${errMsg}`);
	}
	return v;
}
