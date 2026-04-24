// Render a Package-on-disk that uses helm.template / kustomize.build
// — these still need the `akua` CLI on PATH because the embedded
// engines live in the CLI binary, not the WASM bundle. `renderSource`
// covers the pure-KCL cases where no binary is needed.
//
// Run with the CLI built: `task build:engines && cargo install
// --path crates/akua-cli`, or point at your local debug build:
// `AKUA_BINARY=/path/to/target/debug/akua bun run 08-...ts`.

import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { Akua } from '../src/mod.ts';

const PACKAGE = `
schema Input:
    appName: str
    replicas: int = 2

input: Input = option("input") or Input {}

resources = [{
    apiVersion: "v1"
    kind: "ConfigMap"
    metadata.name: input.appName
    data.replicas: str(input.replicas)
}]
`;

const INPUTS = `appName: checkout\nreplicas: 3\n`;

const dir = mkdtempSync(join(tmpdir(), 'akua-shell-render-'));
const out = mkdtempSync(join(tmpdir(), 'akua-shell-render-out-'));
try {
	writeFileSync(join(dir, 'package.k'), PACKAGE);
	writeFileSync(join(dir, 'inputs.yaml'), INPUTS);

	const akua = new Akua({ binary: process.env.AKUA_BINARY ?? 'akua' });
	const summary = await akua.render({
		package: join(dir, 'package.k'),
		inputs: join(dir, 'inputs.yaml'),
		out,
	});

	console.log(`format:    ${summary.format}`);
	console.log(`manifests: ${summary.manifests}`);
	console.log(`hash:      ${summary.hash}`);
	console.log(`target:    ${summary.target}`);
	console.log(`files:     ${summary.files.join(', ')}`);
} finally {
	rmSync(dir, { recursive: true, force: true });
	rmSync(out, { recursive: true, force: true });
}
