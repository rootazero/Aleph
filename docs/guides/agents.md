# Agents Configuration Guide

## File Paths
- Agent definitions: `~/.aleph/config.toml` sections `[agents.defaults]` and `[[agents.list]]`
- Agent workspace: `~/.aleph/workspaces/{agent_id}/`
- Agent data: `~/.aleph/agents/{agent_id}/`

## Operation Rules
1. Before modification: `cp ~/.aleph/config.toml ~/.aleph/config.toml.bak`
2. Use `agent_create` / `agent_delete` tools for creating/deleting agents
3. Edit workspace files (SOUL.md, MEMORY.md) directly — reload on next agent resolution
4. config.toml agent changes auto-reload via fswatch

## Config Structure

```toml
[agents.defaults]
model = "claude-sonnet-4"
workspace_root = "~/.aleph/workspaces"
agents_root = "~/.aleph/agents"
bootstrap_max_chars = 20000

[[agents.list]]
id = "main"                # Required, unique
default = true             # At most one agent
name = "Main Agent"
profile = "default"        # Reference to [profiles.<name>]
model = "claude-opus-4"    # Overrides defaults.model
skills = ["*"]             # Glob patterns; ["*"] = all

[agents.list.identity]
emoji = "🧠"
description = "General-purpose assistant"
```

## Workspace Files

Each agent has a workspace at `~/.aleph/workspaces/{agent_id}/`:

| File | Format | Purpose |
|------|--------|---------|
| `SOUL.md` | Markdown + YAML frontmatter | Core persona (SoulManifest) |
| `AGENTS.md` | Markdown | Operating manual |
| `MEMORY.md` | Markdown | Long-term memory (max 20K chars) |
| `IDENTITY.md` | Markdown | Name, role, vibe |

## SOUL.md Format (SoulManifest)

```markdown
---
relationship: mentor
voice:
  tone: professional and friendly
  verbosity: balanced
expertise:
  - Rust
  - Systems Programming
---

## Identity
I am an expert systems programmer...

## Directives
- Always explain reasoning
- Provide code examples

## Anti-Patterns
- Never make up information
- Avoid unnecessary complexity
```

**Fields:**
- `relationship`: peer | mentor | assistant (default) | expert
- `voice.verbosity`: concise | balanced (default) | elaborate
- `voice.formatting_style`: minimal | markdown (default) | rich

## Common Operations

### Modify agent personality
Edit `~/.aleph/workspaces/{agent_id}/SOUL.md` directly.

### Change agent's model
Edit `model` field in the agent's `[[agents.list]]` entry in config.toml.

### Add agent memory
Edit `~/.aleph/workspaces/{agent_id}/MEMORY.md` (keep under 20K chars).

## Caveats
- Model resolution: agent.model > defaults.model > profile.model > "claude-opus-4-6"
- Skills resolution: agent.skills > defaults.skills > ["*"]
- Use `agent_create` tool for new agents — it handles directory setup + registration
- SOUL.md supports JSON, TOML, YAML, or Markdown with YAML frontmatter
