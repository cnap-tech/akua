import { describe, expect, test } from 'bun:test';

import { Akua } from './mod.ts';
import { MINIMAL_PACKAGE_K } from './test-utils.ts';

describe('Akua.renderSource', () => {
	const akua = new Akua();

	test('default inputs apply the schema default', async () => {
		const yaml = await akua.renderSource('package.k', MINIMAL_PACKAGE_K);
		expect(yaml).toContain('kind: ConfigMap');
		expect(yaml).toMatch(/count:\s*['"]?2['"]?/);
	});

	test('inputs override the schema default', async () => {
		const yaml = await akua.renderSource('package.k', MINIMAL_PACKAGE_K, { replicas: 7 });
		expect(yaml).toMatch(/count:\s*['"]?7['"]?/);
	});

	test('KCL syntax errors surface as thrown exceptions', async () => {
		await expect(akua.renderSource('package.k', 'this is not valid kcl')).rejects.toThrow();
	});
});
