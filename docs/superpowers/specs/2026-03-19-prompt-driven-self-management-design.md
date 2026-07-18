# Prompt-Driven Self-Management Design

**Date:** 2026-03-19
**Status:** Approved
**Motivation:** Replace complex Rust builtin tools for self-management with LLM-driven file operations guided by on-demand configuration knowledge prompts.

## Problem

Aleph has accumulated many builtin tools for self-management (`config_read`, `config_update`, `soul_update`, `profile_update`, `skill_reader`) that are essentially Rust wrappers around file I/O. This violates:
- **R8 (LLM Sovereignty)** — deterministic code replacing LLM's natural ability to read/write structured text
- **R3 (Core Minimalism)** — unnecessary Rust code in core for tasks LLM handles natively
- **R9 (Everything is a Tool)** — but the tools should give LLM capabilities it lacks, not duplicate what it can do

## Solution

A single `read_config_guide(topic)` tool providing progressive disclosure of configuration knowledge, combined with existing file I/O tools. LLM reads the guide, then directly edits config files. Existing `hot_reload.rs` fswatch handles automatic config reload.

## Architecture

```
User: "Configure OpenAI API key"
  ↓
LLM reasoning: needs config knowledge
  ↓ calls read_config_guide("providers")
Guide returned: providers.md (~1500 tokens)
  ↓
LLM executes:
  1. cp config.toml config.toml.bak
  2. vault_store("store", "provider:openai", "sk-xxx")
  3. read config.toml → edit [providers.openai] → write back
  4. fswatch auto-reloads → done
```

## Tool Changes

### New Tools

#### `read_config_guide(topic)`

```rust
read_config_guide(topic: enum {
    "overview",     // All config domains + file paths + basic operation model
    "providers",    // LLM provider config + vault key management
    "mcp",          // mcp_config.json structure + server config
    "skills",       // Skills directory layout + SKILL.md format + manual install
    "agents",       // Agent workspace layout + SOUL.md/MEMORY.md format
    "general",      // general/memory/policies config sections
    "generation",   // Image/speech/video generation provider config
    "channels",     // Telegram/Discord channel config
    "cron",         // Cron task config (pairs with cron_manage tool)
})
```

Implementation: reads pre-written Markdown files from `~/.aleph/guides/{topic}.md`. Source of truth in repo at `docs/guides/`.

#### `vault_store(action, key, secret)`

```rust
vault_store(
    action: enum { "store", "delete", "list" },
    key: String,            // e.g., "provider:openai", "gen:stability"
    secret: Option<String>, // required for "store", ignored otherwise
)
```

Extracted from existing `config_update` and `providers.update` handler. Reuses `SharedTokenManager::store_secret/remove_secret/list_keys`.

Security:
- `list` returns key names only, never secret values
- No `read` action — secrets are write-only
- Each domain-specific guide documents its own vault key naming convention (e.g., `providers.md` documents `provider:{name}`, `generation.md` documents `gen:{name}`)

### Retained Tools (LLM cannot replicate)

| Tool | Reason |
|------|--------|
| `cron_manage` | Runtime IPC with cron service |
| `clawhub` | HTTP requests + ZIP extraction |
| `mcp_manage` (simplified) | MCP server process start/stop/reload |
| `agent_create` / `agent_delete` | Transactional multi-file + registry operations |
| `agent_list` / `agent_switch` | Runtime session state |
| `vault_store` (new) | Encrypted vault binary format, LLM cannot access |

### Removed Tools (LLM replaces with file I/O)

| Tool | Migration |
|------|-----------|
| `config_read` | LLM reads file directly (secrets not in toml) |
| `config_update` | LLM writes file + fswatch auto-reloads |
| `soul_update` | LLM edits SOUL.md directly |
| `profile_update` | LLM edits user_profile.md directly |
| `skill_reader` | Merged into `read_config_guide("skills")` + LLM reads SKILL.md |

### Migration Strategy

Removed tools are **unregistered from tool registry** but code retained in repo. Physical deletion after validation period.

## Guide File Specification

Location: `docs/guides/` (repo) → `~/.aleph/guides/` (runtime). Guides are overwritten from repo on server start — they are not user-editable. Single-user system; no concurrent edit protection needed.

### Template

```markdown
# {Topic} Configuration Guide

## File Paths
- Main: `~/.aleph/config.toml` section `[{section}]`
- (Additional files if any)

## Operation Rules
1. Before modification: `cp {file} {file}.bak`
2. Show planned changes to user and confirm before writing
3. (Topic-specific rules, e.g., secrets via vault_store)
4. After writing: read the file back to verify format is valid
5. config.toml auto-reloads via fswatch, no restart needed

## Structure
(TOML/JSON/YAML structure + field meanings, only user-modifiable fields)

## Common Operations
### Add {xxx}
(Steps)

### Modify {xxx}
(Steps)

### Delete {xxx}
(Steps)

## Caveats
(Domain-specific pitfalls and edge cases)
```

### Content Principles

- Each file ≤ 1500 tokens
- Only what LLM needs to operate, no background prose
- Field descriptions as inline comments, not paragraphs
- Examples use realistic values, not `<placeholder>`
- Updated alongside code changes (same discipline as code)

## System Prompt Integration

No new prompt layer. Add ~60 tokens to `OperationalGuidelinesLayer` (priority 800):

```
You can manage all Aleph configuration. When needed, call read_config_guide(topic)
to get the configuration manual for the relevant domain, then use file read/write
tools to make changes. Always backup config files before modification (cp file file.bak).
Show planned changes to the user and confirm before writing. After writing, read
the file back to verify the format is valid.
API keys must be stored via vault_store tool, never written to config files.
```

Tool schema descriptions:

```
read_config_guide: Get Aleph configuration manual. Call when user needs to modify
config, install plugins/skills, configure API keys, manage agents, or other
self-management operations. Returns structure, steps, and caveats for the domain.

vault_store: Manage encrypted secret vault. API keys and sensitive credentials
must be stored via this tool, never written directly to config files.
```

## Hot Reload

Reuses existing infrastructure:
- `src/gateway/hot_reload.rs` — fswatch on `config.toml`, 500ms debounce, auto-reload to memory
- If TOML parse fails on reload → reload rejected, in-memory config preserved, user restores from `.bak`
- MCP config changes → LLM calls `mcp_manage` to trigger process restart
- Vault changes → immediate (in-memory `SharedTokenManager`)

## Token Budget

- System prompt overhead: ~60 tokens (guideline sentence)
- Per-guide call: ≤ 1500 tokens (via tool result, not system prompt)
- No impact on existing 80,000 char prompt budget
- Guide content never enters system prompt — only flows through tool responses

## Guide Topics (9 files)

1. **overview.md** — File map, operation model, backup rules, hot-reload behavior
2. **providers.md** — `[providers.*]` structure, vault key naming, model binding
3. **mcp.md** — `mcp_config.json` format, env vars, runtime requirements
4. **skills.md** — Directory layout, SKILL.md format, manual install, ClawHub
5. **agents.md** — Workspace layout (`~/.aleph/agents/{id}/`), SOUL.md YAML schema, MEMORY.md
6. **general.md** — `[general]`, `[memory]`, `[policies]`, `[dispatcher]`
7. **generation.md** — `[generation]` providers, vault keys (`gen:{name}`)
8. **channels.md** — `[channels]` opaque JSON config per channel type
9. **cron.md** — `[cron]` config structure (pairs with `cron_manage` tool for runtime ops)
