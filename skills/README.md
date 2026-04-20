# akua skills

Agent-ready workflows for common akua tasks. Each subdirectory is a **skill** — a self-contained package of metadata, instructions, and optional scripts that AI agents can discover, load on demand, and execute.

This directory follows the **[Agent Skills Specification](https://agentskills.io/specification)** — an open standard originally developed by Anthropic and adopted by 35+ agent products (Claude Code, OpenAI Codex, Cursor, Gemini CLI, GitHub Copilot, Amp, Goose, OpenCode, Cline, and more).

---

## The shape

```
skills/
├── <skill-name>/
│   ├── SKILL.md                  # required: YAML frontmatter + instructions
│   ├── scripts/                  # optional: executable helpers
│   ├── references/               # optional: additional docs agent can load on demand
│   └── assets/                   # optional: templates, diagrams, data
└── README.md                     # this file
```

Every skill has a `SKILL.md` with frontmatter (`name` + `description` required, 1024 chars max on description). The `name` must match the parent directory. Body is Markdown with task instructions. Progressive disclosure: agents load metadata (~100 tokens) for all skills; the full body only when activated; references/assets only when needed.

Validation: `npx skills-ref validate ./skills/<skill-name>`

---

## Skills in this repo

| skill | what it does |
|---|---|
| [new-package](new-package/) | Scaffold a new akua Package from scratch |
| [inspect-package](inspect-package/) | Audit a published Package before using it |
| [diff-gate](diff-gate/) | CI gate that blocks breaking package upgrades |
| [dev-loop](dev-loop/) | Sub-second hot-reload against a local cluster |
| [migrate-helmfile](migrate-helmfile/) | Convert helmfile.yaml to an akua Package |
| [rotate-secret](rotate-secret/) | Rotate a shared secret across every install |
| [publish-signed](publish-signed/) | Publish with cosign signature + SLSA attestation |
| [apply-policy-tier](apply-policy-tier/) | Subscribe to and apply a policy tier (soc2, hipaa, …) |

---

## Installing akua skills into your agent

These skills install wherever the Agent Skills Specification is supported. Use whatever your agent provides:

**With `npx skills` (Vercel Labs skill manager):**

```sh
npx skills install github:cnap-tech/akua/skills
```

**With Claude Code:**

```sh
# skills mount automatically from ./skills/ in the current repo, or:
cp -r skills/* ~/.claude/skills/
```

**With Cursor, Codex, Gemini CLI, OpenCode, Amp, Goose, etc.:**

See each agent's skills docs — the `SKILL.md` files here conform to the shared standard and work out of the box.

---

## Why this matters for akua

akua's positioning is "agent-first, humans at checkpoints" (see [docs/cli-contract.md](../docs/cli-contract.md)). Skills are the mechanism through which agents gain task-level competence with akua without us shipping an MCP server (which would burn 30k-90k tokens of context just for tool definitions).

Skills are cheap: ~100 tokens per skill for metadata; full body only loaded when agent decides a skill is relevant. They carry **procedural knowledge** — how to accomplish a class of task — rather than primitive tool calls.

Agents that install these skills gain akua competence without custom integrations. The same skill works in Claude Code, Cursor, Codex, Gemini CLI, Goose, Amp — anywhere the Agent Skills standard is supported.

---

## Contributing a skill

1. Create `skills/<your-skill-name>/SKILL.md` with required frontmatter.
2. `name` matches directory name (lowercase, hyphen-separated).
3. `description` under 1024 chars, includes trigger keywords agents would recognize.
4. Body under 500 lines. Use `references/` for long-form content.
5. Validate: `npx skills-ref validate ./skills/<your-skill-name>`
6. Open a PR. CI runs the validator on every skill.

Don't invent a fork of the spec — stick to what's at [agentskills.io](https://agentskills.io). Interoperability is the point.
