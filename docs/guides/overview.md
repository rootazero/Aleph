# Aleph Configuration Overview

## File Map

| File | Format | Hot-reload | Description |
|------|--------|------------|-------------|
| `~/.aleph/config.toml` | TOML | Yes (fswatch, 500ms debounce) | Main configuration |
| `~/.aleph/mcp_config.json` | JSON | No (call mcp_manage after edit) | MCP server definitions |
| `~/.aleph/agents/{id}/` | Directory | On next agent resolution | Per-agent workspace |
| `~/.aleph/skills/` | Directories | On next skill discovery | Installed skills |
| `~/.aleph/user_profile.md` | Markdown | On next session | User preferences |

## Operation Model

1. **Backup**: `cp ~/.aleph/config.toml ~/.aleph/config.toml.bak`
2. **Read**: Read current file content
3. **Edit**: Make changes, show diff to user, confirm before writing
4. **Verify**: Read back file, check format is valid
5. **Reload**: config.toml auto-reloads; MCP needs `mcp_manage`; agents reload on resolution

## Secret Management

API keys and credentials are stored in an encrypted vault, never in config files.

- **Store**: `vault_store(action="store", key="provider:openai", secret="sk-...")`
- **Delete**: `vault_store(action="delete", key="provider:openai")`
- **List**: `vault_store(action="list")` — returns key names only, never values

Key naming conventions:
- LLM providers: `provider:{name}` (e.g., `provider:openai`)
- Generation providers: `gen:{name}` (e.g., `gen:stability`)
- Channels: `channel:{type}:{id}` (e.g., `channel:telegram:bot1`)

## Config Sections (config.toml)

Call `read_config_guide(topic)` for details on each:

- `providers` — LLM provider configs (OpenAI, Claude, Gemini, Ollama)
- `general` — Default provider, language, queue mode
- `memory` — Vector DB, embedding, retrieval settings
- `generation` — Image/speech/video generation providers
- `channels` — Telegram, Discord, etc.
- `profiles` — Workspace profiles (model, tools, system prompt)
- `agents` — Agent definitions and defaults
- `cron` — Scheduled task config
- `policies` — Tool safety, retry, filtering policies
