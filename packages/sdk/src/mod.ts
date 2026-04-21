// Spike scaffold for @akua/sdk. One verb (`version`) wired end-to-end
// to prove the pattern — spawn `akua <verb> --json`, parse stdout,
// return a value typed by the ts-rs-generated types. Errors are parsed
// from stderr (StructuredError JSONL) + exit code, classified into
// the hierarchy in ./errors.ts.
//
// Types import directly from the repo's neutral `sdk-types/` dir during
// the spike. Publish-time packaging (copy/symlink into `packages/sdk/`,
// emit `.d.ts`, drop the `.ts` extension per Node 22 NodeNext) is a
// follow-up concern, not part of this scaffold.

import { execFile } from 'node:child_process';

import type { VersionOutput } from '../../../sdk-types/VersionOutput.ts';

import { AkuaTransportError, classifyCliError } from './errors.ts';

export * from './errors.ts';
export type { VersionOutput };
export type { ExitCode } from '../../../sdk-types/ExitCode.ts';
export type { StructuredError, Level } from '../../../sdk-types/StructuredError.ts';

export interface AkuaOptions {
	/** Path to the `akua` binary. Defaults to `"akua"` (resolved via PATH). */
	binary?: string;
}

interface ExecResult {
	stdout: string;
	stderr: string;
}

/**
 * Thin wrapper around the `akua` CLI. Each method shells out to a verb,
 * parses the `--json` output, and returns a value whose shape comes from
 * the Rust serde types (generated via ts-rs). Failures throw the right
 * `AkuaError` subclass based on exit code + parsed StructuredError.
 */
export class Akua {
	readonly binary: string;

	constructor(opts: AkuaOptions = {}) {
		this.binary = opts.binary ?? 'akua';
	}

	async version(): Promise<VersionOutput> {
		const { stdout } = await this.runJson(['version', '--json']);
		return JSON.parse(stdout) as VersionOutput;
	}

	// ---- internals ----

	private runJson(args: readonly string[]): Promise<ExecResult> {
		return new Promise((resolve, reject) => {
			const child = execFile(
				this.binary,
				args,
				{ encoding: 'utf8' },
				(err, stdout, stderr) => {
					if (!err) {
						resolve({ stdout, stderr });
						return;
					}
					// `spawn` itself failed — ENOENT, EACCES, etc. — there's no exitCode.
					if ((err as NodeJS.ErrnoException).code === 'ENOENT') {
						reject(
							new AkuaTransportError(`akua binary not found: ${this.binary}`, { cause: err }),
						);
						return;
					}
					reject(
						classifyCliError({
							rawExitCode: child.exitCode,
							stderr,
							cause: err,
						}),
					);
				},
			);
		});
	}
}
