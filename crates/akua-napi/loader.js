// Loader wrapper for @akua-dev/native.
//
// The per-platform .node binaries ship without embedded helm /
// kustomize engine wasm (built with `--no-default-features`,
// `embed-engines` OFF). The Rust side then expects the bytes to come
// from disk via the `AKUA_NATIVE_ENGINES_DIR` env var. This wrapper:
//
//   1. Resolves @akua-dev/native-engines to its on-disk directory.
//   2. Sets process.env.AKUA_NATIVE_ENGINES_DIR before the per-
//      platform addon's first require() call.
//   3. Re-exports everything from the auto-generated index.js (which
//      napi-rs's `napi prepublish` produces and we don't hand-edit).
//
// Why a separate file: index.js is regenerated on every `napi build`,
// so any setup we'd inline there gets clobbered. Keeping the env-var
// plumbing in loader.js makes it survive regen + leaves the auto-gen
// machinery untouched. See cnap-tech/akua#482.

'use strict';

const path = require('node:path');

const ENV_VAR = 'AKUA_NATIVE_ENGINES_DIR';

// Don't override an explicit user setting (CI test fixtures, vendored
// engines, etc.). When the env var is already populated, trust it.
if (!process.env[ENV_VAR]) {
	try {
		// `require.resolve` returns the path to the package's main
		// file; the engines package's directory is its parent.
		const enginesPkgEntry = require.resolve('@akua-dev/native-engines');
		process.env[ENV_VAR] = path.dirname(enginesPkgEntry);
	} catch (e) {
		// `@akua-dev/native-engines` is a hard dependency of
		// `@akua-dev/native` (declared in package.json), so missing
		// it means the consumer's install is broken — typically
		// `npm install` skipped optionalDependencies, or a tool
		// hand-vendored the addon without bringing engines along.
		// Warn loudly: the Rust side will still surface its own
		// E_ENGINE_NOT_AVAILABLE on the first helm.template /
		// kustomize.build call, but that's a render-time error a
		// loader-time warning prevents.
		const msg = e && e.message ? e.message : String(e);
		// eslint-disable-next-line no-console
		console.warn(
			`@akua-dev/native: could not locate @akua-dev/native-engines (${msg}). ` +
				`Reinstall with \`npm install @akua-dev/native\` (engines must come along), ` +
				`or set ${ENV_VAR} explicitly to a directory containing helm-engine.wasm + kustomize-engine.wasm. ` +
				`Renders that don't call helm.template / kustomize.build will still work.`,
		);
	}
}

module.exports = require('./index.js');
