---
name: self
description: "Aleph self-management mode — configure LLM providers, generation providers (image/video/speech/audio), channels (Telegram/Discord), agents, skills, plugins, MCP servers, and other system settings via config.toml and vault. Use when the user asks to add/modify/remove a provider, change settings, install a skill or plugin, configure a channel, manage API keys, or any task involving ~/.aleph/ configuration. Also triggered by explicit /self command."
---

# Aleph Self-Management

## Critical Rules

1. **Use `self_config` tool for config changes** — preferred over bash for structured access with validation.
2. **Use `bash` for raw file access** — `file_ops` cannot access `~/.aleph/config.toml` (denied_paths).
3. **Store API key FIRST, then edit config** — `vault_store`, then `bash`. Never write keys to config files.
4. **Copy exact TOML field names** — wrong field names silently fail. Refer to reference docs.
5. **TOML section ordering** — subsections like `[generation.providers.X]` must appear after `[generation]` and before the next top-level section.
6. **Generation providers need restart** — after adding/modifying, tell user to restart Aleph.
7. **LLM providers default disabled** — always set `enabled = true` explicitly.
8. **PII filtering** — if user's API key shows as `[REDACTED]`, ask them to use `vault_store` directly.
9. **Kill before restart** — multiple aleph processes corrupt vault. See [config-editing.md](references/config-editing.md).

## Tools

| Tool | Use for |
|------|---------|
| `vault_store` | Store/delete/list API keys |
| `self_config` | Read/update config with validation + natural language preview |
| `bash` | Raw file read (cat ~/.aleph/config.toml), system commands |
| `read_config_guide` | Load detailed guide for a domain (see topics below) |
| `web_fetch` / `search` | Only for plugin/skill installs needing external docs |

**Never use**: `file_ops` (denied_paths), `image_generate`, `generate_video`.

---

## self_config Tool

The `self_config` tool provides structured access to identity files and config.toml with built-in validation and natural language preview.

### Actions

#### ListFiles
List all identity files and their status.
```
self_config(action="ListFiles")
```

#### ReadFile
Read an identity file by name.
```
self_config(action="ReadFile", file_name="MEMORY.md")
```
Allowed files: `MEMORY.md`, `SOUL.md`, `AGENTS.md`, `IDENTITY.md`, `TOOLS.md`, `HEARTBEAT.md`

#### WriteFile
Write content to an identity file (creates if not exists).
```
self_config(action="WriteFile", file_name="SOUL.md", content="# My Soul\n\nI am...")
```

#### ReadConfig
Read a config section as JSON using dot-path syntax.
```
self_config(action="ReadConfig", config_path="providers.openai")
self_config(action="ReadConfig", config_path="general")
```

#### UpdateConfig (核心功能)
Update a config section via deep-merge patch with optional preview.

**Preview mode (dry_run=true)** — 查看变更但不写入：
```
self_config(
  action="UpdateConfig",
  config_path="providers.openai",
  config_value={
    "enabled": true,
    "model": "gpt-4-turbo",
    "temperature": 0.9
  },
  dry_run=true
)
```

**Apply mode (dry_run=false)** — 确认后应用更改：
```
self_config(
  action="UpdateConfig",
  config_path="providers.openai",
  config_value={
    "enabled": true,
    "model": "gpt-4-turbo",
    "temperature": 0.9
  },
  dry_run=false
)
```

### Preview Response Example

当 `dry_run=true` 时，响应包含 `preview_message` 字段：

```json
{
  "success": true,
  "message": "Config patch dry-run at 'providers.openai' (3 changes)",
  "data": {
    "applied_sections": ["providers"],
    "diff": [
      {"path": "providers.openai.enabled", "old_value": null, "new_value": true},
      {"path": "providers.openai.model", "old_value": null, "new_value": "gpt-4-turbo"},
      {"path": "providers.openai.temperature", "old_value": 0.7, "new_value": 0.9}
    ]
  },
  "preview_message": "将为 'providers.openai' 做出以下更改：\n• 新增字段: providers.openai.enabled = true\n• 新增字段: providers.openai.model = \"gpt-4-turbo\"\n• 修改字段: providers.openai.temperature: 0.7 → 0.9\n\n此为预览模式，未写入配置文件。确认后将以 dry_run=false 再次调用以应用更改。"
}
```

### Recommended Workflow for Config Changes

```
1. Read current config: self_config(action="ReadConfig", config_path="providers.openai")
   ↓
2. Preview changes:    self_config(action="UpdateConfig", ..., dry_run=true)
   ↓
3. Show preview_message to user and ask for confirmation
   ↓
4. Apply changes:      self_config(action="UpdateConfig", ..., dry_run=false)
```

### Config Path Examples

| Path | Meaning |
|------|---------|
| `providers` | All LLM providers |
| `providers.openai` | OpenAI provider settings |
| `providers.deepseek.model` | DeepSeek model field |
| `general` | General settings (language, default_provider) |
| `memory` | Memory system config |
| `channels.discord` | Discord channel config |

---

## Operation Protocol

### For Secret/API Key Changes

1. Store secret: `vault_store(action="store", key="provider:openai", secret="sk-...")`
2. Read config: `self_config(action="ReadConfig", config_path="providers.openai")`
3. Update config: `self_config(action="UpdateConfig", config_path="providers.openai", config_value={...}, dry_run=true)`
4. Apply: `self_config(action="UpdateConfig", ..., dry_run=false)`

### For Complex Config Edits (via bash)

1. Read current: `bash(cat ~/.aleph/config.toml)`
2. Edit with python3:
```python
import toml
config = toml.load(open("~/.aleph/config.toml"))
# modify config
with open("~/.aleph/config.toml", "w") as f:
    toml.dump(config, f)
```
3. Verify: `bash(grep -A10 '\[providers.openai\]' ~/.aleph/config.toml)`

---

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

---

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

---

## Workspace: ~/.aleph/

```
~/.aleph/
├── config.toml              # Main config (hot-reload)
├── soul.md                  # Global persona
├── user_profile.md          # User profile
├── mcp_config.json          # MCP server definitions
├── agents/{id}/             # Agent data (SOUL.md, MEMORY.md, sessions/)
├── workspaces/{id}/output/ # Agent file output
├── skills/                  # All skills (official + user-installed)
├── plugins/                 # Installed plugins
├── data/                    # LanceDB, vault, sessions DB
└── output/                  # Global default output
```

---

## Reference Docs

Read the appropriate reference file when working on a specific domain:

- **[LLM Providers](references/llm-providers.md)** — protocols, presets, base_url rules, full template, field pitfalls. Read when adding/modifying an LLM provider in `[providers.*]`.
- **[Generation Providers](references/generation-providers.md)** — provider_type values, ResolvedUrl rules, templates, defaults block, typed maps. Read when adding/modifying image/video/speech/audio providers in `[generation.*]`.
- **[Channels](references/channels.md)** — Telegram, Discord TOML templates and all fields. Read when configuring a channel in `[channels.*]`.
- **[Extensions](references/extensions.md)** — plugin CLI commands, skill format, MCP .mcp.json, installation and errors. Read when installing/managing plugins, skills, or MCP servers.
- **[Config Editing](references/config-editing.md)** — TOML editing rules, section order, process management. Read when performing complex config edits or restarting Aleph.
