/**
 * @akua/mcp — Model Context Protocol server for Akua.
 *
 * Exposes Akua's package tools to AI coding agents (Claude Code, Cursor,
 * Claude Desktop, any MCP-compatible host):
 *
 * - `pkg.introspect` — describe schema, user-input fields, component layout
 * - `pkg.preview` — resolve values + render manifests for a given input set
 * - `pkg.test` — run resolve.test.* and report results
 * - `pkg.validate` — schema + transform output validation
 * - `pkg.build` — produce an OCI-ready artifact
 *
 * Status: pre-alpha. Tool registration scaffold only.
 *
 * @example
 * ```bash
 * akua mcp            # run via the CNAP CLI
 * # or:
 * npx @akua/mcp       # run standalone
 * ```
 */

export interface McpToolDescriptor {
	name: string;
	description: string;
}

export const TOOLS: McpToolDescriptor[] = [
	{ name: 'pkg.introspect', description: 'Describe a package schema and user-input fields.' },
	{
		name: 'pkg.preview',
		description: 'Resolve values and render K8s manifests for a given input set.'
	},
	{ name: 'pkg.test', description: 'Run package tests and return results.' },
	{ name: 'pkg.validate', description: 'Validate schema and transform output.' },
	{ name: 'pkg.build', description: 'Build the package into an OCI-ready artifact.' }
];
