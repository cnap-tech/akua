// End-to-end test for `Akua.render(opts)` — shells out to a freshly
// built `akua` binary against a scratch Package and validates the
// returned `RenderSummary`. Gated on `AKUA_E2E=1`.

import { describe, expect, test } from 'bun:test';
import { join } from 'node:path';

import { Akua } from './mod.ts';
import { BINARY, E2E_ENABLED, MINIMAL_PACKAGE_K, scratchPackageWith } from './test-utils.ts';

describe.if(E2E_ENABLED)('Akua.render', () => {
	test('renders a minimal package and returns a typed summary', async () => {
		using pkg = scratchPackageWith(MINIMAL_PACKAGE_K, 'akua-sdk-render-');
		const akua = new Akua({ binary: BINARY });
		const summary = await akua.render({
			package: join(pkg.dir, 'package.k'),
			out: join(pkg.dir, 'deploy'),
		});
		expect(summary.format).toBe('raw-manifests');
		expect(summary.manifests).toBe(1);
		expect(summary.hash).toMatch(/^sha256:/);
		expect(summary.target).toContain('deploy');
	});

	test('--dry-run skips writing but still returns a summary', async () => {
		using pkg = scratchPackageWith(MINIMAL_PACKAGE_K, 'akua-sdk-render-');
		const akua = new Akua({ binary: BINARY });
		const summary = await akua.render({
			package: join(pkg.dir, 'package.k'),
			out: join(pkg.dir, 'deploy'),
			dryRun: true,
		});
		expect(summary.manifests).toBe(1);
	});
});
