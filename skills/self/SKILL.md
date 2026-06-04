---
name: self
description: "Aleph self-management mode — configure LLM providers, generation providers (image/video/speech/audio), channels (Telegram/Discord/15+), agents, skills, plugins, MCP servers, cron jobs, and other system settings via config.toml and vault. Use when the user asks to add/modify/remove a provider, change settings, install a skill or plugin, configure a channel, manage API keys, or any task involving ~/.aleph/ configuration. Also triggered by explicit /self command."
---

# Aleph Self-Management

## Critical Rules

1. **Use `self_config` for config changes** — structured access with validation and preview. Prefer over bash.
2. **Use `bash` for raw file access** — `file_ops` cannot access `~/.aleph/config.toml` (denied_paths).
3. **Store API key FIRST, then edit config** — `vault_store`, then `self_config`. Never write keys to config files.
4. **Copy exact TOML field names** — wrong field names silently fail. Refer to reference docs.
5. **TOML section ordering** — subsections like `[generation.providers.X]` must appear after `[generation]` and before the next top-level section.
6. **Generation providers need restart** — after adding/modifying, tell user to restart Aleph.
7. **LLM providers default disabled** — always set `enabled = true` explicitly.
8. **Kill before restart** — multiple aleph processes corrupt vault. See [config-editing.md](references/config-editing.md).

## Tools

| Tool | Use for |
|------|---------|
| `vault_store` | Store/delete/list API keys |
| `self_config` | Read/write identity files and config.toml with validation + preview |
| `agent_create` / `agent_delete` / `agent_list` / `agent_info` | Agent lifecycle management |
| `channel_pairing` | Generate/list pairing codes (Telegram) |
| `remember` | Add/replace/remove entries in MEMORY.md (curated memory) |
| `read_config_guide` | Load detailed guide for a domain |
| `bash` | Raw file read, python3 edits, system commands |
| `web_fetch` / `search` | External docs for plugin/skill installs only |

**Never use**: `file_ops` (denied_paths), `image_generate`, `video_generate`.

---

## Identity Files

Stored in `~/.aleph/agents/{agent_id}/`, injected into the agent's system prompt on each turn.

| File | Read | Write | Purpose |
|------|------|-------|---------|
| `SOUL.md` | self_config | self_config | Core persona |
| `IDENTITY.md` | self_config | self_config | Identity definition |
| `AGENTS.md` | self_config | self_config | Agent behavior guide |
| `TOOLS.md` | self_config | self_config | Tool usage guide |
| `HEARTBEAT.md` | self_config | self_config | Health check guide |

> **Note**: `MEMORY.md` is managed by the `remember` tool (action=`add`/`replace`/`remove`). It is **not** accessible via `self_config`. Use `remember` for all MEMORY.md operations.

---

## self_config Tool

Structured access to identity files and config.toml with built-in validation and natural language preview.

### Actions

> **Action values are snake_case** — the `action` field accepts `list_files`,
> `read_file`, `write_file`, `read_config`, `update_config`, `route_status`.
> PascalCase spellings (e.g. `ListFiles`) fail deserialization.

#### list_files
List all identity files and their status (exists, size).
```
self_config(action="list_files")
```

#### read_file
Read an identity file by name.
```
self_config(action="read_file", file_name="SOUL.md")
```
Allowed files: `SOUL.md`, `IDENTITY.md`, `AGENTS.md`, `TOOLS.md`, `HEARTBEAT.md`

#### write_file
Write content to an identity file (creates if not exists). Changes take effect on the next turn.
```
self_config(action="write_file", file_name="SOUL.md", content="# My Soul\n\nI am...")
```

#### read_config
Read a config section as JSON using dot-path syntax.
```
self_config(action="read_config", config_path="providers.openai")
self_config(action="read_config", config_path="general")
```

#### update_config
Update a config section via deep-merge patch with optional preview.

**Preview mode (dry_run=true)** — show changes without writing:
```
self_config(
  action="update_config",
  config_path="providers.openai",
  config_value={
    "enabled": true,
    "models": ["gpt-4o"],
    "temperature": 0.9
  },
  dry_run=true
)
```

**Apply mode (dry_run=false)** — apply after user confirms:
```
self_config(
  action="update_config",
  config_path="providers.openai",
  config_value={
    "enabled": true,
    "models": ["gpt-4o"],
    "temperature": 0.9
  },
  dry_run=false
)
```

### Preview Response

When `dry_run=true`, the response includes a `preview_message` in Chinese:

```json
{
  "success": true,
  "message": "Config patch dry-run at 'providers.openai' (3 changes)",
  "data": {
    "applied_sections": ["providers"],
    "diff": [
      {"path": "providers.openai.enabled", "old_value": null, "new_value": true},
      {"path": "providers.openai.models", "old_value": null, "new_value": ["gpt-4o"]},
      {"path": "providers.openai.temperature", "old_value": 0.7, "new_value": 0.9}
    ]
  },
  "preview_message": "将为 'providers.openai' 做出以下更改：\n• 新增字段: providers.openai.enabled = true\n• 新增字段: providers.openai.models = [\"gpt-4o\"]\n• 修改字段: providers.openai.temperature: 0.7 → 0.9\n\n此为预览模式，未写入配置文件。确认后将以 dry_run=false 再次调用以应用更改。"
}
```

#### route_status
Show the current local/cloud route decision (`mode` + `allow_cloud_escalation`).
Read-only view; to *change* the route, patch `config_path="route"`.
```
self_config(action="route_status")
self_config(action="update_config", config_path="route", config_value={"mode": "always_local"}, dry_run=false)
```
A `route` patch hot-applies to the live failover chain on the next prompt — no restart needed.

### Recommended Workflow

```
1. Read current:    self_config(action="read_config", config_path="providers.openai")
   ↓
2. Preview changes: self_config(action="update_config", ..., dry_run=true)
   ↓
3. Show preview_message to user and ask for confirmation
   ↓
4. Apply changes:   self_config(action="update_config", ..., dry_run=false)
```

### Config Path Examples

| Path | Meaning |
|------|---------|
| `providers` | All LLM providers |
| `providers.openai` | OpenAI provider settings |
| `providers.deepseek.models` | DeepSeek models array (first = default) |
| `general` | General settings (language, default_provider) |
| `memory` | Memory system config |
| `channels.discord` | Discord channel config |
| `generation` | Generation provider config |
| `cron` | Scheduled tasks |
| `profiles` | Workspace profiles (Anti-Gravity Architecture) |
| `secret_providers` | Secret backend configs (1Password, Bitwarden, local vault) |
| `secrets` | Logical secret name → provider mappings |
| `secrets_config` | Top-level secret subsystem settings |
| `execution` | Agent timeout and iteration limits |
| `orchestrator` | Orchestrator guard limits (rounds, tool calls, tokens) |
| `stop_hooks` | Pre-stop shell command hooks |
| `policies` | Behavioral policies (tool safety, retry, intent, etc.) |
| `sandbox` | Sandbox runtime config (workspace root, timeout, output cap) |
| `agents` | Agent definitions and global defaults |
| `agent` (alias `cowork`) | Agent task orchestration settings |
| `bindings` | Channel → Agent routing bindings |
| `route` | Local-vs-cloud routing mode (`mode`, `allow_cloud_escalation`). Prefer the `route_status` action to view; patch this path to change. |

---

## Agent Management Tools

| Tool | Purpose | Confirmation |
|------|---------|-------------|
| `agent_create` | Create a new agent with workspace and memory | No |
| `agent_delete` | Delete an agent (cannot delete "main") | **Yes** |
| `agent_list` | List all agents, show active one | No |
| `agent_info` | Get agent capabilities, config, tools | No |

### Agent Creation Example
```
agent_create(id="trader", name="Trading Assistant", description="Stock analysis", model="claude-sonnet-4-5")
```

> After creating an agent, bind it to channels via `bindings` in config.toml or the Gateway routing system.

---

## Channel Pairing

```
channel_pairing(action="generate")                    # Generate pairing code
channel_pairing(action="generate", channel_id="telegram")
channel_pairing(action="list")                        # List active codes
```

Use when the user wants to link a Telegram account to Aleph. (Pairing-code
resolution currently targets Telegram channels only.)

---

## Config Sections Quick Reference

All top-level sections available in `config.toml`:

| Section | Purpose | Key Fields |
|---------|---------|-----------|
| `general` | Core settings | `default_provider`, `language`, `log_level` |
| `memory` | Cognitive memory | `enabled`, `embedding`, `auto_save` |
| `providers.*` | LLM providers | `protocol`, `models`, `enabled`, `base_url` |
| `rules` | Provider routing rules | `pattern`, `provider`, `priority` |
| `behavior` | Agent behavior tuning | `auto_confirm`, `streaming`, `verbosity` |
| `search` | Search provider config | `provider`, `api_key_ref`, `max_results` |
| `skills` | Skill system | `enabled`, `skills_dir`, `auto_match_enabled` |
| `tools` | Built-in tools | `enabled`, `timeout_seconds`, `per_tool_overrides` |
| `mcp` | MCP servers | `enabled`, `external_servers` |
| `unified_tools` | Unified tool facade | `enabled`, `registry_path` (takes precedence over legacy `[tools]` + `[mcp]`) |
| `tool_service` | Tool service runtime | `default_timeout`, `max_concurrent` |
| `sandbox` | Execution sandbox | `workspace_root`, `timeout_seconds`, `output_cap_bytes` |
| `smart_flow` | ⚠️ Legacy — parsed but inert (no runtime consumer) | `enabled`, `context_window_target` |
| `smart_matching` | ⚠️ Legacy — parsed but inert (no runtime consumer) | `enabled`, `threshold` |
| `dispatcher` | ⚠️ Legacy — dissolved in the Dispatcher-dissolution refactor (R7); parsed but inert | `enabled`, `fallback_policy` |
| `agent` (alias `cowork`) | Agent task orchestration | `enabled`, `max_subagents`, `default_timeout` |
| `policies` | Behavioral policies | `tool_safety`, `retry`, `intent`, `memory.compression` |
| `generation` | Media generation | `providers`, `image_providers`, `video_providers`, `speech_providers`, `audio_providers` |
| `orchestrator` | Three-Layer Orchestrator | `guards.max_rounds`, `guards.max_tool_calls`, `guards.max_tokens` |
| `subagent` | ⚠️ Legacy — parsed but inert (no runtime consumer) | `enabled`, `max_depth`, `inherit_context` |
| `task_routing` | ⚠️ Legacy — parsed but inert (no runtime consumer) | `enabled`, `default_queue`, `strategies` |
| `route` | Local-vs-cloud failover routing (LIVE — shapes the failover chain by endpoint tier) | `mode` (`auto`/`always_local`/`always_cloud`), `allow_cloud_escalation`, `local_provider`, `cloud_provider` |
| `group_chat` | Multi-agent chat | `enabled`, `personas`, `rotation_strategy` |
| `cron` | Scheduler runtime (jobs themselves are DB-backed, managed via the `cron_manage` tool) | `enabled`, `db_path`, `check_interval_secs`, `max_concurrent_jobs`, `job_timeout_secs` |
| `heartbeat` | Health monitoring | `enabled`, `interval_seconds`, `endpoints` |
| `personas` | Preset persona definitions | `name`, `system_prompt`, `model` |
| `evolution` | ⚠️ Legacy — parsed but inert (no runtime consumer) | `enabled`, `auto_generate`, `threshold` |
| `media` | ⚠️ Legacy — parsed but inert (no runtime consumer) | `enabled`, `providers`, `cache_ttl` |
| `privacy` | PII filtering | `enabled`, `redaction_level`, `allowed_entities` |
| `security` | Shell security | `shell.enable_custom_patterns`, `shell.custom_blocked` |
| `ssrf` | SSRF protection | `enabled`, `allowed_hosts`, `blocked_hosts` |
| `profiles.*` | Workspace profiles | `model`, `tools`, `system_prompt`, `temperature` |
| `secret_providers.*` | Secret backends | `type` (`local_vault`, `1password`, `bitwarden`) |
| `secrets.*` | Secret mappings | `provider`, `ref`, `sensitivity`, `ttl` |
| `secrets_config` | Secret defaults | `default_provider`, `virtual_keys` |
| `prompt` | Prompt customization | `extra_files`, `inject_order` |
| `channels.*` | Channel instances | Platform-specific fields (Telegram, Discord, etc.) |
| `a2a` | A2A protocol | `enabled`, `endpoint`, `auth` |
| `acp` | ACP harness | `enabled`, `mode`, `timeout_seconds` |
| `execution` | Execution engine | `default_timeout_secs` (default 48h), `max_iterations` (default 1000), `prompt_mode`, `progress_push` |
| `agents` | Agent definitions | `defaults`, `list` (array of agent definitions) |
| `bindings` | Route bindings | `channel`, `pattern`, `agent_id` |
| `plugin_marketplaces.*` | Plugin sources | `source`, `type` (`github` or `local`) |
| `stop_hooks` | Stop hooks | `name`, `command`, `timeout_secs` |
| `guardrails` | PII/secret hard-filter switch | `enabled` |
| `stability` | P0 rescue knobs (stall watchdog, failure cap, per-turn timeout) | `stall_timeout_secs`, `consecutive_failure_cap`, `turn_timeout_secs` |
| `fallback_provider` | Provider failover chain | `chain`, `provider` |
| `context_budget` | Mid-run context-window compaction | `enabled`, `token_budget`, `warning_threshold`, `critical_threshold` |
| `resume` | Boot-scan auto-resume of interrupted runs | `enabled`, `max_age_secs`, `max_attempts`, `max_concurrent` |
| `projects` | Project workspace registry (per-directory skills/memory/plugins/hooks scoping) | `enabled`, project roots |
| `tasks_reaper` (alias `task_reaper`) | Background reaper for stale/orphaned tasks | `enabled`, sweep interval |

> **Top-level scalar**: `default_hotkey` (string) sits at the config root, not inside any section.
>
> **Important**: `[agent]` (task orchestration) and `[agents]` (agent definitions) are **different sections**. Do not confuse them.

---

## Operation Protocol

### For Secret/API Key Changes

1. Store secret: `vault_store(action="store", key="ai:openai", secret="sk-...")`
2. Read config: `self_config(action="read_config", config_path="providers.openai")`
3. Preview: `self_config(action="update_config", config_path="providers.openai", config_value={...}, dry_run=true)`
4. Apply: `self_config(action="update_config", ..., dry_run=false)`

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
| LLM providers | `ai:{name}` | `ai:openai` |
| Generation providers | `gen:{name}` | `gen:stability` |
| Channels | `channel:{instance_id}:{secret_field}` | `channel:telegram:bot_token` |
| Embedding | `embed:{id}` | `embed:openai` |
| Rerank | `rerank:{provider}` | `rerank:cohere` |
| Search backends | `search:{name}` | `search:brave` |

> **Vault prefix is `ai:` not `provider:`** — the runtime reads LLM provider keys
> only from `ai:{name}`. A key stored under `provider:openai` is never read and
> provider auth fails silently. Embeddings likewise read `embed:{id}`, not
> `embedding:{name}`.

---

## Curated Memory (MEMORY.md)

Use the `remember` tool for entry-level edits:

```
remember(action="add", content="User prefers concise responses")
remember(action="replace", old_text="concise", content="User prefers detailed responses")
remember(action="remove", old_text="concise")
```

- `remember(action="add", ...)` — append new fact
- `remember(action="replace", ...)` — replace existing entry
- `remember(action="remove", ...)` — remove entry

---

## read_config_guide Topics

| Topic | Covers |
|-------|--------|
| overview | File map, operation model, all sections |
| providers | LLM provider config + vault |
| generation | Image/speech/video/audio providers |
| channels | Telegram, Discord, and 15+ channel configs |
| agents | Agent workspace, SOUL.md |
| skills | Skill install, format |
| mcp | MCP server config |
| general | Default provider, language, policies |
| cron | Scheduled tasks |

---

## Workspace: ~/.aleph/

```
~/.aleph/
├── config.toml              # Main config (hot-reload, fswatch 500ms)
├── defaults.toml            # Default overrides (not serialized to config.toml)
├── presets.toml             # Preset overrides
├── prompts.toml             # Prompt overrides
├── soul.md                  # Global persona
├── user_profile.md          # User profile
├── mcp_config.json          # MCP server definitions
├── agents/{id}/             # Agent identity files (SOUL.md, IDENTITY.md, AGENTS.md, TOOLS.md, HEARTBEAT.md)
│   └── sessions/            # Agent session data
├── workspaces/{id}/output/  # Agent runtime tool output and scratch files
├── skills/                  # All skills (official + user-installed)
├── plugins/                 # Installed plugins
├── data/                    # LanceDB, vault, sessions DB
└── output/                  # Global default output
```

> **Identity files** (SOUL.md, IDENTITY.md, AGENTS.md, TOOLS.md, HEARTBEAT.md) live under `~/.aleph/agents/{agent_id}/` and are injected into the agent's system prompt on each turn.
>
> **MEMORY.md** is managed by the curated memory module (`src/memory/curated/`), not by identity files. Use the `remember` tool for memory operations.

---

## Override Config Files

These files live alongside `config.toml` and are loaded at runtime. They are **not** managed by `self_config` (which only patches `config.toml`). Use `bash` to edit them directly.

| File | Purpose | Editable via self_config |
|------|---------|------------------------|
| `~/.aleph/defaults.toml` | Override default values for all config fields | No |
| `~/.aleph/presets.toml` | Provider preset overrides (base_url, protocol, color, model) | No |
| `~/.aleph/prompts.toml` | Custom prompt injections and system prompt overrides | No |

**Example `defaults.toml`**:
```toml
[provider_defaults]
timeout_seconds = 600
color = "#10a37f"

[memory_defaults]
embedding_batch_size = 32
```

---

## Advanced: Secrets Management

Beyond `vault_store`, Aleph supports external secret providers for team/enterprise use.

### Secret Providers

```toml
[secrets_config]
default_provider = "local"

[secret_providers.local]
type = "local_vault"

[secret_providers.op]
type = "1password"
account = "my.1password.com"
service_account_token_env = "OP_SERVICE_ACCOUNT_TOKEN"
```

Supported provider types: `local_vault`, `1password`, `bitwarden`.

### Secret Mappings

Map logical names to provider references:

```toml
[secrets.OPENAI_API_KEY]
provider = "op"
ref = "OpenAI/api-key"
sensitivity = "high"
ttl = 1800
```

| Field | Description |
|-------|-------------|
| `provider` | Name of the secret provider (must match a `secret_providers` key) |
| `ref` | Provider-specific reference path (e.g., `"OpenAI/api-key"` for 1Password) |
| `sensitivity` | `"standard"` or `"high"` — affects cache duration and redaction |
| `ttl` | Cache time-to-live in seconds (default: 3600) |

---

## Advanced: Workspace Profiles

Profiles (Anti-Gravity Architecture) define the "physics" of a workspace — model binding, tool whitelist, and system prompt. They are static templates; workspaces are runtime instances.

```toml
[profiles.coding]
description = "Rust/Python development"
model = "claude-sonnet-4"
tools = ["git_*", "fs_*", "terminal", "search"]
system_prompt = "You are a senior engineer..."
temperature = 0.2
history_limit = 50

[profiles.creative]
description = "Creative writing"
model = "gemini-2.5-flash"
tools = ["search", "fs_read"]
temperature = 0.9
```

| Field | Description |
|-------|-------------|
| `model` | Bound AI model (overrides general default) |
| `tools` | Tool whitelist using glob patterns (e.g., `git_*`, `fs_*`) |
| `system_prompt` | Additional system prompt appended to base |
| `temperature` | Generation temperature (0.0–2.0) |
| `max_tokens` | Max response tokens |
| `history_limit` | Max messages to retain in context |

---

## Advanced: Reliability & Rescue Sections

Opt-in sections that harden long-running Think→Act loops. Each is **absent by
default** — omitting the section keeps the feature off. These wire into the
harness at startup, so tell the user to restart Aleph after changing them.

```toml
# PII / secret hard-filter on input + output + tool calls
[guardrails]
enabled = true

# P0 rescue knobs — every field is independently optional
[stability]
stall_timeout_secs = 300           # abort a run with no progress for this long
stall_check_interval_secs = 30     # watchdog poll interval (default 30)
consecutive_failure_cap = 8        # abort after N back-to-back tool failures
turn_timeout_secs = 300            # per-turn wall-clock cap

# Provider failover — references existing [providers.*] keys by name
[fallback_provider]
chain = ["openai", "gemini"]       # tried in order after the primary
# provider = "openai"              # back-compat single-provider form

# Mid-run context-window compaction (prevents context-length hard-fail)
[context_budget]
enabled = true
token_budget = 200000              # your model's real context window
warning_threshold = 0.70           # fraction at which compaction begins
critical_threshold = 0.85          # fraction forcing a final reply

# Boot-scan auto-resume of runs interrupted by a crash/restart
[resume]
enabled = true                     # default true
max_age_secs = 86400               # skip runs older than this (default 24h)
max_attempts = 3                   # abandon after N crash-loops
max_concurrent = 4                 # cap simultaneous resumes at boot
```

---

## Reference Docs

Read the appropriate reference file when working on a specific domain:

- **[LLM Providers](references/llm-providers.md)** — protocols, presets, base_url rules, full template, field pitfalls. Read when adding/modifying an LLM provider in `[providers.*]`.
- **[Generation Providers](references/generation-providers.md)** — provider_type values, ResolvedUrl rules, templates, defaults block, typed maps. Read when adding/modifying image/video/speech/audio providers in `[generation.*]`.
- **[Channels](references/channels.md)** — Telegram, Discord, webhook TOML templates and all fields. Read when configuring a channel in `[channels.*]`.
- **[Extensions](references/extensions.md)** — plugin CLI commands, skill format, MCP .mcp.json, installation and errors. Read when installing/managing plugins, skills, or MCP servers.
- **[Config Editing](references/config-editing.md)** — TOML editing rules, section order, process management. Read when performing complex config edits or restarting Aleph.
