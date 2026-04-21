// Spike scaffold for @akua/sdk. One verb (`version`) wired end-to-end
// to prove the pattern — spawn `akua <verb> --json`, parse stdout,
// return a value typed by the ts-rs-generated types.
//
// Types import directly from the repo's neutral `sdk-types/` dir during
// the spike. Publish-time packaging (copy/symlink into `packages/sdk/`,
// emit `.d.ts`, drop the `.ts` extension per Node 22 NodeNext) is a
// follow-up concern, not part of this scaffold.

import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

import type { VersionOutput } from '../../../sdk-types/VersionOutput.ts';

const run = promisify(execFile);

export interface AkuaOptions {
	/** Path to the `akua` binary. Defaults to `"akua"` (resolved via PATH). */
	binary?: string;
}

/**
 * Thin wrapper around the `akua` CLI. Each method shells out to a verb,
 * parses the `--json` output, and returns a value whose shape comes from
 * the Rust serde types (generated via ts-rs).
 */
export class Akua {
	readonly binary: string;

	constructor(opts: AkuaOptions = {}) {
		this.binary = opts.binary ?? 'akua';
	}

	async version(): Promise<VersionOutput> {
		const { stdout } = await run(this.binary, ['version', '--json']);
		return JSON.parse(stdout) as VersionOutput;
	}
}

export type { VersionOutput };
