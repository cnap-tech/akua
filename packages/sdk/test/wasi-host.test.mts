// Node-only integration tests for the WASI worker host that bridges
// helm + kustomize engines. Bun's `node:wasi` doesn't yet pipe
// `stdin: <fd>` correctly (#464); under Bun this would fail before
// the bridge fires. The SDK's `renderSource()` only routes through
// the WASI worker when the source mentions a plugin — pure-KCL
// renders go through `akua-wasm` (Bun-compatible). So this suite
// exercises the engine path specifically.
//
// Run:  node --test packages/sdk/test/wasi-host.test.mjs

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { Akua } from '../src/mod.ts';

// Bun ≤1.3.7 has incomplete `node:wasi` (stdin fd doesn't pipe;
// tracked in #464). The WASI worker path needs Node — skip the
// whole suite under Bun. The pure-KCL renders in `renderSource.test.ts`
// still cover the akua-wasm path under Bun.
const isBun = typeof globalThis.Bun !== 'undefined';
const t = isBun ? test.skip : test;

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, '../../..');

t('helm.template renders examples/00-helm-hello via in-process WASI host', async () => {
	const akua = new Akua();
	const yaml = await akua.renderSource({
		package: resolve(REPO_ROOT, 'examples/00-helm-hello/package.k'),
	});
	assert.match(yaml, /apiVersion:\s*v1/);
	assert.match(yaml, /kind:\s*ConfigMap/);
});

t('kustomize.build renders examples/09-kustomize-hello via in-process WASI host', async () => {
	const akua = new Akua();
	const yaml = await akua.renderSource({
		package: resolve(REPO_ROOT, 'examples/09-kustomize-hello/package.k'),
	});
	// Example overlays a base ConfigMap with a `prod-` namePrefix +
	// `env: prod` label.
	assert.match(yaml, /kind:\s*ConfigMap/);
	assert.match(yaml, /name:\s*prod-hello/);
	assert.match(yaml, /env:\s*prod/);
});

t('host rejects helm.template with an out-of-package chart path', async () => {
	const akua = new Akua();
	await assert.rejects(
		akua.renderSource({
			source: `
import akua.helm
resources = helm.template(helm.Template {
    chart = "./does-not-exist"
})
`,
			packageDir: REPO_ROOT,
		}),
	);
});
