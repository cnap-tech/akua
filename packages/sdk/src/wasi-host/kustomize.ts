// `kustomize.build` plugin handler — JS-side. Mirrors
// `crates/akua-core/src/kustomize.rs`: validates the overlay path,
// tarballs the overlay's PARENT (so `../base` references inside the
// overlay resolve), invokes the engine, splits the multi-doc YAML
// into a resource list.

import { basename, dirname } from 'node:path';

import type { Engine } from './engine.ts';
import { bytesToBase64 } from './_encoding.ts';
import { extractOptions, requireStringField } from './_options.ts';
import { resolveDirInPackage } from './_path.ts';
import { tarGzipDir } from './tarball.ts';
import { splitYamlDocs } from './yaml-multidoc.ts';

const PLUGIN_NAME = 'kustomize.build';

interface BuildResponse {
	yaml?: string;
	error?: string;
}

interface KustomizeHandlerContext {
	engine: Engine;
	packageDir: string;
}

export function makeKustomizeBuildHandler(ctx: KustomizeHandlerContext) {
	return (argsJson: string, _kwargsJson: string): unknown[] => {
		const args = JSON.parse(argsJson) as unknown[];
		const opts = extractOptions(args, PLUGIN_NAME, 'kustomize.Build');

		const overlayPath = requireStringField(opts, 'path', PLUGIN_NAME, 'options.path must be a string');
		const resolved = resolveDirInPackage(PLUGIN_NAME, ctx.packageDir, overlayPath, 'overlay');

		// Mirror crates/kustomize-engine-wasm/src/lib.rs::render_dir:
		// tar the overlay's PARENT (so `../base` references inside the
		// overlay resolve), pass the entrypoint as
		// `<parent_name>/<overlay_name>` inside the archive.
		const overlayName = basename(resolved);
		const parent = dirname(resolved);
		const parentName = basename(parent);
		const tarGz = tarGzipDir(parent, parentName);
		const guestEntrypoint = `${parentName}/${overlayName}`;

		const request = {
			overlay_tar_gz_b64: bytesToBase64(tarGz),
			entrypoint: guestEntrypoint,
		};
		const input = new TextEncoder().encode(JSON.stringify(request));
		const output = ctx.engine.call(input);
		const response = JSON.parse(new TextDecoder().decode(output)) as BuildResponse;

		if (response.error) {
			throw new Error(`${PLUGIN_NAME}: kustomize engine: ${response.error}`);
		}
		return splitYamlDocs(response.yaml ?? '');
	};
}
