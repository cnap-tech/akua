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
		// Fall through silently — the Rust side has its own
		// fallback to embedded bytes when `embed-engines` is on, and
		// surfaces a clear plugin-panic error when it isn't and no
		// engines are reachable. Don't pre-empt that with a confusing
		// loader-time throw.
	}
}

module.exports = require('./index.js');
