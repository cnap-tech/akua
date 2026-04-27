// Multi-doc YAML splitter — every Kubernetes-shaped engine emits
// a `---`-separated stream, one resource per doc. Mirrors
// `crates/akua-core/src/yaml_multidoc.rs::parse`: empty separator
// docs are dropped so callers can splat the result into KCL's
// `resources` list.

import { parseAllDocuments } from 'yaml';

/**
 * Split a multi-doc YAML string into one parsed value per non-empty
 * document. Null / empty-mapping docs are dropped.
 */
export function splitYamlDocs(text: string): unknown[] {
	const out: unknown[] = [];
	for (const doc of parseAllDocuments(text)) {
		const value = doc.toJS();
		if (isEmptyDoc(value)) continue;
		out.push(value);
	}
	return out;
}

function isEmptyDoc(v: unknown): boolean {
	if (v === null || v === undefined) return true;
	if (typeof v === 'object' && !Array.isArray(v) && Object.keys(v as object).length === 0) {
		return true;
	}
	return false;
}
