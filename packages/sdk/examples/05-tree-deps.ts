// Walk a workspace's declared deps + lockfile entries — what
// charts are pinned, what digests, what (if any) fork-overrides.
// Mirrors `akua tree --json` exactly; runs in-process via WASM.

import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { Akua } from '../src/mod.ts';

const MANIFEST = `[package]
name = "demo"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
nginx = { oci = "oci://registry-1.docker.io/bitnamicharts/nginx", version = "18.2.0" }
local = { path = "../vendor/local" }
`;

const LOCK = `version = 1

[[package]]
name      = "nginx"
version   = "18.2.0"
source    = "oci://registry-1.docker.io/bitnamicharts/nginx"
digest    = "sha256:6ec9b69bfeac053a4c39393169053cef8e7062274221db4139b73140b7302793"
signature = "cosign:sigstore:bitnamicharts"
`;

const dir = mkdtempSync(join(tmpdir(), 'akua-tree-example-'));
try {
	writeFileSync(join(dir, 'akua.toml'), MANIFEST);
	writeFileSync(join(dir, 'akua.lock'), LOCK);

	const akua = new Akua();
	const out = await akua.tree({ workspace: dir });

	console.log(`${out.package.name}@${out.package.version} (${out.dependencies.length} deps)`);
	for (const dep of out.dependencies) {
		const locked = dep.locked ? ` [${dep.locked.digest.slice(0, 17)}…]` : ' [unlocked]';
		console.log(`  - ${dep.name} (${dep.source} ${dep.source_ref})${locked}`);
	}
} finally {
	rmSync(dir, { recursive: true, force: true });
}
