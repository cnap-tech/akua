// `@akua-dev/native-engines` — wasm bytes for helm + kustomize.
//
// This package's job is to be on disk somewhere reachable. The
// `@akua-dev/native` loader resolves us via `require.resolve` and
// sets `AKUA_NATIVE_ENGINES_DIR` to our directory, after which the
// per-platform .node binary reads `helm-engine.wasm` +
// `kustomize-engine.wasm` from there.
//
// The exported object is informational — consumers don't import any
// runtime API from this package, but `require('@akua-dev/native-engines')`
// must succeed for the resolution dance to work.

'use strict';

const path = require('node:path');

module.exports = {
	dir: __dirname,
	files: {
		helm: path.join(__dirname, 'helm-engine.wasm'),
		kustomize: path.join(__dirname, 'kustomize-engine.wasm'),
	},
};
