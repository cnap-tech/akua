// `helm.template` plugin handler — JS-side. Mirrors
// `crates/akua-core/src/helm.rs` end-to-end: validates the chart
// path, tarballs the chart, hands it to the engine, returns a
// resource-list value KCL splices into `resources = ...`.
//
// Critical invariants kept identical to the Rust path so SDK + CLI
// produce byte-equivalent renders:
// - chart path is relative + non-escaping (see `_path.ts`)
// - release name 1..=53 lowercase alnum + `-`, first char alnum
// - namespace 1..=63 lowercase alnum + `-`, first char alnum

import type { Engine } from './engine.ts';
import { bytesToBase64 } from './_encoding.ts';
import { extractOptions, requireStringField } from './_options.ts';
import { resolveDirInPackage } from './_path.ts';
import { tarGzipDir } from './tarball.ts';
import { splitYamlDocs } from './yaml-multidoc.ts';

const PLUGIN_NAME = 'helm.template';

interface Release {
	name: string;
	namespace: string;
	revision: number;
	service: string;
}

interface RenderResponse {
	manifests?: Record<string, string>;
	error?: string;
}

interface HelmHandlerContext {
	engine: Engine;
	/** Absolute path of the calling Package's directory. */
	packageDir: string;
}

export function makeHelmTemplateHandler(ctx: HelmHandlerContext) {
	return (argsJson: string, _kwargsJson: string): unknown[] => {
		const args = JSON.parse(argsJson) as unknown[];
		const opts = extractOptions(args, PLUGIN_NAME, 'helm.Template');

		const chartPath = requireStringField(opts, 'chart', PLUGIN_NAME, 'options.chart must be a string');
		const resolvedChart = resolveDirInPackage(PLUGIN_NAME, ctx.packageDir, chartPath, 'chart');

		const valuesObj =
			(opts.values && typeof opts.values === 'object' && !Array.isArray(opts.values)
				? (opts.values as Record<string, unknown>)
				: {}) ?? {};
		// Helm's engine accepts JSON-shaped YAML for values — same
		// shape `serde_yaml::to_string` over a serde_json::Value
		// produces on the CLI side. No JS YAML emitter needed.
		const valuesYaml = JSON.stringify(valuesObj, null, 2);

		const releaseName = (opts.release as string | undefined) ?? 'release';
		const releaseNamespace = (opts.namespace as string | undefined) ?? 'default';
		validateReleaseName(releaseName);
		validateNamespace(releaseNamespace);

		const chartName = chartDirName(resolvedChart);
		const release: Release = {
			name: releaseName,
			namespace: releaseNamespace,
			revision: 1,
			service: 'Helm',
		};

		const tarGz = tarGzipDir(resolvedChart, chartName);
		const request = {
			chart_tar_gz_b64: bytesToBase64(tarGz),
			values_yaml: valuesYaml,
			release,
		};
		const input = new TextEncoder().encode(JSON.stringify(request));
		const output = ctx.engine.call(input);
		const response = JSON.parse(new TextDecoder().decode(output)) as RenderResponse;

		if (response.error) {
			throw new Error(`${PLUGIN_NAME}: helm engine: ${response.error}`);
		}

		// Each manifest may carry multiple `---`-separated docs (one
		// chart file → N resources). Flatten + drop empty separator
		// docs, mirroring the Rust path through `yaml_multidoc::parse`.
		const resources: unknown[] = [];
		for (const yaml of Object.values(response.manifests ?? {})) {
			for (const doc of splitYamlDocs(yaml)) {
				resources.push(doc);
			}
		}
		return resources;
	};
}

function chartDirName(absChart: string): string {
	return absChart.split(/[\\/]/).filter(Boolean).pop() ?? 'chart';
}

function validateReleaseName(name: string): void {
	if (name.length === 0 || name.length > 53) {
		throw new Error(`${PLUGIN_NAME}: release name \`${name}\` must be 1..=53 chars`);
	}
	if (!/^[a-z0-9][a-z0-9-]*$/.test(name)) {
		throw new Error(
			`${PLUGIN_NAME}: release name \`${name}\` must be lowercase alphanumeric + \`-\`, first char alphanumeric`,
		);
	}
}

function validateNamespace(ns: string): void {
	if (ns.length === 0 || ns.length > 63) {
		throw new Error(`${PLUGIN_NAME}: namespace \`${ns}\` must be 1..=63 chars`);
	}
	if (!/^[a-z0-9][a-z0-9-]*$/.test(ns)) {
		throw new Error(
			`${PLUGIN_NAME}: namespace \`${ns}\` must be lowercase alphanumeric + \`-\`, first char alphanumeric`,
		);
	}
}

// Re-exports for tests that need to drive the validators directly.
export const _internals = {
	validateReleaseName,
	validateNamespace,
	chartDirName,
};
