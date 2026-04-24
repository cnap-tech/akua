import { execFile } from 'node:child_process';

import type { VersionOutput } from './types/VersionOutput.ts';
import type { WhoamiOutput } from './types/WhoamiOutput.ts';

import { classifyCliError } from './errors.ts';
import { type SchemaName, validateAs } from './validate.ts';

export * from './errors.ts';
export { AkuaContractError, standardSchemaFor, validateAs } from './validate.ts';
export type { SchemaName } from './validate.ts';
export type { VersionOutput, WhoamiOutput };
export type { ExitCode } from './types/ExitCode.ts';
export type { StructuredError, Level } from './types/StructuredError.ts';
export type { AgentContext } from './types/AgentContext.ts';
export type { AgentSource } from './types/AgentSource.ts';

// 64 MiB covers large `inspect` / `render` outputs. Verbs that stream
// multi-hundred-MiB artifacts should use a streaming API instead of
// buffering into a single string (not this class).
const DEFAULT_MAX_BUFFER = 64 * 1024 * 1024;

export interface AkuaOptions {
	/** Path to the `akua` binary. Defaults to `"akua"` (resolved via PATH). */
	binary?: string;
}

/**
 * Thin wrapper around the `akua` CLI. Each method shells out to a verb,
 * parses the `--json` output, and returns a value typed by the ts-rs
 * generated types. Failures throw the right `AkuaError` subclass based
 * on exit code + parsed StructuredError.
 */
export class Akua {
	readonly binary: string;

	constructor(opts: AkuaOptions = {}) {
		this.binary = opts.binary ?? 'akua';
	}

	version(): Promise<VersionOutput> {
		return this.call('version', 'VersionOutput');
	}

	whoami(): Promise<WhoamiOutput> {
		return this.call('whoami', 'WhoamiOutput');
	}

	private async call<T>(verb: string, schema: SchemaName): Promise<T> {
		const { stdout } = await this.exec([verb, '--json']);
		return validateAs<T>(schema, JSON.parse(stdout));
	}

	private exec(args: readonly string[]): Promise<{ stdout: string; stderr: string }> {
		return new Promise((resolve, reject) => {
			const child = execFile(
				this.binary,
				args,
				{ encoding: 'utf8', maxBuffer: DEFAULT_MAX_BUFFER },
				(err, stdout, stderr) => {
					if (!err) {
						resolve({ stdout, stderr });
						return;
					}
					// Both binary-not-found (ENOENT, exitCode == null) and non-zero
					// exits land here — `classifyCliError` picks the right subclass.
					reject(classifyCliError({ rawExitCode: child.exitCode, stderr, cause: err }));
				},
			);
		});
	}
}
