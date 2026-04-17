/**
 * @akua/sdk — high-level TypeScript API for Akua.
 *
 * Typed functions for building, previewing, testing, and publishing
 * cloud-native packages. Wraps @akua/core (NAPI/WASM bindings to the
 * Rust pipeline).
 *
 * Status: pre-alpha. API will change.
 *
 * @example
 * ```ts
 * import { Package, preview, buildPackage } from '@akua/sdk';
 *
 * const pkg = await Package.load('./my-package');
 * const result = await preview(pkg, { inputs: { subdomain: 'acme' } });
 * console.log(result.manifests);
 * ```
 */

export interface PackageManifest {
	name: string;
	version: string;
	components: Component[];
	userInputs?: unknown; // JSON Schema Draft 7 with x-user-input / x-input
	transforms?: TransformRef[];
}

export interface Component {
	name: string;
	type: 'helm-chart' | 'knative-app' | 'oci-registry' | 'git-repo' | 'raw-manifests';
	source: string;
	config?: Record<string, unknown>;
}

export interface TransformRef {
	name: string;
	path: string; // resolve.ts, resolve.wasm, etc.
}

export interface PreviewInputs {
	[key: string]: unknown;
}

export interface PreviewResult {
	values: Record<string, unknown>;
	manifests: string; // rendered K8s YAML
	errors: string[];
}

export class Package {
	static async load(_path: string): Promise<Package> {
		throw new Error('Package.load — not yet implemented (milestone v4)');
	}
}

export async function preview(_pkg: Package, _inputs: PreviewInputs): Promise<PreviewResult> {
	throw new Error('preview — not yet implemented (milestone v4)');
}

export async function buildPackage(_pkg: Package): Promise<{ ociRef: string }> {
	throw new Error('buildPackage — not yet implemented (milestone v4)');
}
