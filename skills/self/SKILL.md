---
name: self
description: "Aleph self-management mode — configure LLM providers, generation providers (image/video/speech/audio), channels (Telegram/Discord), agents, skills, plugins, MCP servers, and other system settings via config.toml and vault. Use when the user asks to add/modify/remove a provider, change settings, install a skill or plugin, configure a channel, manage API keys, or any task involving ~/.aleph/ configuration. Also triggered by explicit /self command."
---

# Aleph Self-Management

## Critical Rules

1. **Use `bash` for config files** — `file_ops` cannot access `~/.aleph/config.toml` (denied_paths).
2. **Store API key FIRST, then edit config** — `vault_store`, then `bash`. Never write keys to config files.
3. **Copy exact TOML field names** — wrong field names silently fail. Refer to reference docs.
4. **TOML section ordering** — subsections like `[generation.providers.X]` must appear after `[generation]` and before the next top-level section.
5. **Generation providers need restart** — after adding/modifying, tell user to restart Aleph.
6. **LLM providers default disabled** — always set `enabled = true` explicitly.
7. **PII filtering** — if user's API key shows as `[REDACTED]`, ask them to use `vault_store` directly.
8. **Kill before restart** — multiple aleph processes corrupt vault. See [config-editing.md](references/config-editing.md).

## Tools

| Tool | Use for |
|------|---------|
| `vault_store` | Store/delete/list API keys |
| `bash` | Read/write config files, run system commands |
| `read_config_guide` | Load detailed guide for a domain (see topics below) |
| `web_fetch` / `search` | Only for plugin/skill installs needing external docs |

**Never use**: `file_ops` (denied_paths), `image_generate`, `generate_video`.

## Operation Protocol

1. Store secrets: `vault_store(action="store", key="<convention>", secret="<key>")`
2. Read config: `bash(cat ~/.aleph/config.toml)`
3. Edit config: `bash` with python3 (safer than sed for TOML)
4. Verify: `bash(grep -A10 'section_name' ~/.aleph/config.toml)`
5. Config auto-reloads (fswatch 500ms) — no restart needed except generation providers.

## Secret Management

```
vault_store(action="store", key="<convention>", secret="<api_key>")
vault_store(action="delete", key="<convention>")
vault_store(action="list")
```

| Type | Key Convention | Example |
|------|---------------|---------|
| LLM providers | `provider:{name}` | `provider:openai` |
| Generation providers | `gen:{name}` | `gen:T8StarVideo` |
| Channels | `channel:{type}:{id}` | `channel:telegram:bot1` |
| Embedding | `embedding:{name}` | `embedding:openai` |

## read_config_guide Topics

| Topic | Covers |
|-------|--------|
| overview | File map, operation model, all sections |
| providers | LLM provider config + vault |
| generation | Image/speech/video/audio providers |
| channels | Telegram, Discord config |
| agents | Agent workspace, SOUL.md |
| skills | Skill install, format |
| mcp | MCP server config |
| general | Default provider, language, policies |
| cron | Scheduled tasks |

## Workspace: ~/.aleph/

```
~/.aleph/
├── config.toml              # Main config (hot-reload)
├── soul.md                  # Global persona
├── user_profile.md          # User profile
├── mcp_config.json          # MCP server definitions
├── agents/{id}/             # Agent data (SOUL.md, MEMORY.md, sessions/)
├── workspaces/{id}/output/  # Agent file output
├── skills/                  # User custom skills
├── skills-official/         # Official skills (read-only)
├── plugins/                 # Installed plugins
├── data/                    # LanceDB, vault, sessions DB
└── output/                  # Global default output
```

## Reference Docs

Read the appropriate reference file when working on a specific domain:

- **[LLM Providers](references/llm-providers.md)** — protocols, presets, base_url rules, full template, field pitfalls. Read when adding/modifying an LLM provider in `[providers.*]`.
- **[Generation Providers](references/generation-providers.md)** — provider_type values, ResolvedUrl rules, templates, defaults block, typed maps. Read when adding/modifying image/video/speech/audio providers in `[generation.*]`.
- **[Channels](references/channels.md)** — Telegram, Discord TOML templates and all fields. Read when configuring a channel in `[channels.*]`.
- **[Extensions](references/extensions.md)** — plugin CLI commands, skill format, MCP .mcp.json, installation and errors. Read when installing/managing plugins, skills, or MCP servers.
- **[Config Editing](references/config-editing.md)** — TOML editing rules, section order, process management. Read when performing complex config edits or restarting Aleph.
