# Using akua with AI agents

akua is designed agent-first. This doc covers how agents discover akua's capabilities, what ships out of the box, and why the architecture is the way it is.

---

## The short version

- akua auto-detects agent sessions from standard env vars (`AGENT=…`, `CLAUDECODE=1`, etc.) and silently enables JSON output, structured errors, no-interactive, no-color. See [cli-contract.md §1.5](cli-contract.md#15-agent-context-auto-detection).
- The akua CLI surface follows a strict contract (JSON-first, typed exit codes, idempotent writes, plan mode, time-bounded) — the CLI is the agent API. See [cli-contract.md](cli-contract.md).
- Agent-ready workflows ship as skills in [`skills/`](../skills/) following the open [Agent Skills Specification](https://agentskills.io). Install them into Claude Code, Cursor, Codex, Gemini CLI, Goose, Amp, OpenCode, or any of the 35+ supported agents.
- No MCP server. Shell + skills is structurally more efficient than MCP for operational tools (token cost, composability).

---

## Why no MCP server?

MCP tool definitions consume 30k–90k tokens of agent context per connection before any reasoning starts. For a CLI with 20 verbs and 100+ subcommands, that's catastrophic.

The alternative the ecosystem is converging on — pioneered by Cloudflare, validated by Google Workspace CLI — is:

1. **CLI surface with JSON-first output** (structured data for the agent; legible for humans).
2. **Skills in the repository** (natural-language task descriptions the agent loads on-demand; ~100 tokens of metadata at startup, full body only when needed).
3. **Auto-detection of agent context** so the agent never has to remember `--json` or similar agent-specific flags.

This combination matches how agents already work. Agents compose through shell pipes; they read markdown. They don't need a separate protocol for each CLI.

---

## How akua auto-detects agent context

At process start, akua checks environment variables in this order:

| env var | agent |
|---|---|
| `AGENT=<name>` | Goose, Amp, Codex, Cline, OpenCode (standard) |
| `CLAUDECODE=1` | Claude Code |
| `GEMINI_CLI=1` | Gemini CLI |
| `CURSOR_CLI=1` | Cursor CLI |
| `AKUA_AGENT=<name>` | akua-specific fallback |

If any matches, akua silently enables `--json`, `--log=json`, `--no-color`, `--no-progress`, `--no-interactive`. Explicit flags always win (user can force text output with `--no-json` or `--format=text`).

No stderr announcement. No prelude on stdout. Detection is observable via `akua whoami --json` (reveals the `agent_context` field) or at `--log-level=debug`. Otherwise invisible.

---

## How to install akua skills into your agent

### Claude Code

Skills under `skills/` in any project Claude Code opens are discovered automatically. No installation needed. Alternatively for global access:

```sh
cp -r path/to/akua/skills/* ~/.claude/skills/
```

### OpenAI Codex

Install via the Codex skills manager:

```sh
codex skills install github:cnap-tech/akua/skills
```

### Cursor

Add `skills/` to Cursor's skill paths in `.cursor/config.json`:

```json
{ "skills": { "paths": ["./skills"] } }
```

### Gemini CLI

Install as a Gemini CLI extension:

```sh
gemini extensions install @akua/skills
```

### Goose, Amp, OpenCode, Cline, Roo Code, Amp, Command Code, Kiro, Factory, and 25+ others

All support the open [Agent Skills Specification](https://agentskills.io). Any of:

- Symlink `skills/` into the agent's expected location
- Use `npx skills install github:cnap-tech/akua/skills`
- Follow each agent's skill-installation instructions (linked from [agentskills.io/overview](https://agentskills.io/))

### Universal: `npx skills`

The [Vercel Labs skills manager](https://github.com/vercel-labs/skills) works across all Agent Skills compatible agents:

```sh
npx skills install github:cnap-tech/akua/skills
npx skills list
npx skills remove akua-*
```

---

## Shipped skills

Eight initial skills covering the most common akua workflows. See [`skills/`](../skills/) for details.

| skill | use when |
|---|---|
| [new-package](../skills/new-package/) | user wants to start a new akua Package |
| [inspect-package](../skills/inspect-package/) | auditing a third-party Package before use |
| [diff-gate](../skills/diff-gate/) | setting up CI to block breaking upgrades |
| [dev-loop](../skills/dev-loop/) | iterating on a Package with hot-reload |
| [migrate-helmfile](../skills/migrate-helmfile/) | converting Helmfile to akua |
| [rotate-secret](../skills/rotate-secret/) | rotating a shared secret across installs |
| [publish-signed](../skills/publish-signed/) | releasing a signed + attested Package |
| [apply-policy-tier](../skills/apply-policy-tier/) | subscribing to a compliance / production tier |

---

## Writing your own skill

Follow [the spec](https://agentskills.io/specification):

```
skills/my-skill/
├── SKILL.md              # required
├── scripts/              # optional — helper scripts
├── references/           # optional — long-form docs
└── assets/               # optional — templates, diagrams
```

`SKILL.md` minimum:

```markdown
---
name: my-skill
description: What this does and when to use it.
---

# My skill

Step-by-step instructions...
```

Validation: `npx skills-ref validate ./skills/my-skill`

Good descriptions include trigger keywords agents would recognize. Agents load metadata for all skills (~100 tokens each) at startup; they load the full body only when they decide a skill applies.

See the [shipped skills](../skills/) for canonical examples.

---

## Running agents against akua — example loop

```
agent receives user intent:
  "add a Redis to my checkout service"

agent loads skills metadata:
  reads ~100 tokens per skill, selects new-package + inspect-package

agent loads new-package SKILL.md fully:
  now has full procedure for scaffolding + adding sources

agent executes:
  $ akua add chart oci://ghcr.io/bitnami/charts/redis --version 21.0.0
  $ edit package.k to wire redis values to existing schema
  $ akua lint
  $ akua render --inputs inputs.yaml --out ./rendered

agent verifies:
  $ akua diff previous:v1.2 ./rendered --json
  (structural diff shows: new source redis, new schema field redis.replicas)

agent commits + opens PR:
  $ git commit -am "feat: add redis to checkout"
  $ gh pr create

CI runs:
  akua lint + diff-gate + policy check → attached to PR as comments

human reviews + approves + merges

deploy repo auto-updates; ArgoCD syncs
```

The whole loop: ~300 tokens of agent context for metadata, ~1000-2000 for the activated skill, plus primary task context. No MCP, no separate protocol, no magic — shell + git + markdown.

---

## Related reading

- [cli-contract.md](cli-contract.md) — the CLI invariants agents rely on
- [cli.md](cli.md) — full verb reference
- [sdk.md](sdk.md) — programmatic access if you prefer SDK over CLI
- [agentskills.io](https://agentskills.io) — the skill-format standard
- [Cloudflare: code execution > MCP tools for agent ops](https://blog.cloudflare.com/code-mode-the-better-way-to-use-mcp/) — the research that shifted the ecosystem
