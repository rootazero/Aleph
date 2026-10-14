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

**Verify mode (verify=true)** — for `providers.*` changes, probe the provider's
live reachability right after applying, so you confirm the new key/endpoint
actually works in the same call:
```
self_config(
  action="update_config",
  config_path="providers.openai",
  config_value={ "enabled": true, "models": ["gpt-4o"] },
  dry_run=false,
  verify=true
)
```
The response `message` then reports "Provider connectivity verified: reachable"
or "Provider connectivity FAILED: …", and `data.health_check` is
`passed` / `{"failed": {"reason": …}}` / `skipped`. `verify` is ignored for
non-provider paths and in `dry_run`. On FAILED, refresh the key with
`vault_store` (`ai:<name>`) or fix the `providers.<name>` section, then retry.

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

### Reload Impact (when does the change take effect?)

Every `update_config` response (both preview and apply) now carries a
**`reload_impact`** signal in `data` and in the `message`. Trust it instead of
guessing — a config patch only refreshes the in-memory config + the file on
disk; most subsystems captured their config at startup and will not see the
change until restart.

| `kind` | Meaning | What to tell the user |
|--------|---------|-----------------------|
| `live` | Hot-swapped onto the running runtime (only `route` today) | Effective next prompt, no restart |
| `restart` | Saved, but runtime captured it at startup | **Tell the user to restart aleph-server** |
| `inert` | Legacy section, no runtime consumer | Saved but changes nothing — confirm intent |

> The old per-section "needs restart" notes below (generation, the reliability
> sections, etc.) are the same rules this signal encodes — read it off the tool
> response rather than memorizing them.

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
| `secrets_config` | Secret subsystem settings (virtual-key aliases, custom leak patterns) |
| `execution` | Agent timeout and iteration limits |
| `stop_hooks` | Pre-stop shell command hooks |
| `policies` | Behavioral policies (tool safety, retry, intent, etc.) |
| `sandbox` | Sandbox runtime config (workspace root, timeout, output cap) |
| `agents` | Agent definitions and global defaults |
| `bindings` | Channel → Agent routing bindings |
| `route` | Local-vs-cloud routing mode (`mode`, `allow_cloud_escalation`). Prefer the `route_status` action to view; patch this path to change. |
| `session` | DM scope (`dm_scope` = `per-peer` / `main` / `per-channel-peer`) |
| `gateway` | Gateway server (host, port, TLS, origins) — read by `GatewayConfig`, not by `Config` |

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
| `general` | Core settings | `default_provider`, `fallback_providers`, `language`, `browser` |
| `memory` | Cognitive memory | `enabled`, `embedding`, `curated`, `dreaming` |
| `providers.*` | LLM providers | `protocol`, `models`, `enabled`, `base_url` |
| `rules` | Custom **slash commands only**, not provider routing: an entry whose `regex` starts with `^/` becomes a tool named after the command, and anything else is inert. See *Advanced: Slash-Command Rules* below before writing one. | `regex`, `system_prompt`, `is_builtin` |
| `behavior` | Output rendering mode | `output_mode` (`typewriter` or `instant`; the only field `BehaviorConfig` declares) |
| `search` | Search provider config | `enabled`, `default_provider`, `max_results`, `backends.*` (`provider_type`, `api_key`, `base_url`) |
| `fetch` | URL→markdown fetch backends. **Not wired into `web_fetch` at runtime** (BT-D-R4-22: a provider resolves the URL from its own network position, so the SSRF DNS pin cannot be enforced) — `web_fetch` always uses the built-in pinned fetch, and a configured backend only triggers a one-time startup warning. The section is still read by the `fetch_config.test` connection-test RPC, so credentials here are testable but not used for real fetches. | `enabled`, `default_provider`, `fallback_providers`, `backends.*` |
| `tools` | Built-in tools | `fs_enabled`, `shell_enabled`, `git_enabled`, `allowed_roots`, `allowed_commands`, `core`, `defer_mcp_tools` |
| `mcp` | MCP servers | `enabled`, `external_servers` |
| `unified_tools` | Unified tool facade | `enabled`, `native`, `mcp` (takes precedence over legacy `[tools]` + `[mcp]`) |
| `tool_service` | Tool service runtime | `default_timeout_seconds`, `per_tool_seconds`, `parallel_tool_concurrency` |
| `sandbox` | Execution sandbox | `enabled`, `workspace_root`, `default_timeout_seconds`, `max_output_bytes`, `command_policy` (hard floor — no session knob overrides it) |
| `policies` | Behavioral policies | `tool_permissions`, `exec_tier`, `spend`, `mode`, `memory.compression`, `web_fetch` |
| `generation` | Media generation | `providers`, `image_providers`, `video_providers`, `speech_providers`, `audio_providers` |
| `voice` | Local voice — BYO OpenAI-compatible STT/TTS (`[voice.local]`, `[voice.streaming]`, `[voice.format]`) | `local`, `streaming`, `format`, `llm_provider`, `llm_model` |
| `route` | Local-vs-cloud failover routing (LIVE — shapes the failover chain by endpoint tier) | `mode` (`auto`/`always_local`/`always_cloud`), `allow_cloud_escalation`, `local_provider`, `cloud_provider` |
| `group_chat` | Multi-agent chat | `max_rounds`, `max_personas_per_session`, `history_window_rounds`, `coordinator_visible` |
| `cron` | Scheduler runtime (jobs themselves are DB-backed, managed via the `cron_manage` tool) | `enabled`, `db_path`, `check_interval_secs`, `max_concurrent_agents`, `job_timeout_secs` |
| `heartbeat` | Health monitoring | `enabled`, `tick_interval_secs`, `max_concurrent`, `job_timeout_secs`, `dedup` |
| `personas` | Preset persona definitions | `id`, `name`, `system_prompt`, `provider`, `model` |
| `privacy` | PII filtering | `pii_filtering`, `email`, `phone`, `api_key`, `custom_rules`, `exclude_providers` |
| `security` | Shell security (flat table — there is **no** `[security.shell]` sub-table) | `enable_custom_patterns`, `custom_blocked`, `custom_danger`, `mask_patterns` |
| `ssrf` | SSRF protection | `enabled`, `allowed_hosts`, `blocked_hosts` |
| `profiles.*` | Workspace profiles | `model`, `smart_recall.*` (the only two knobs `ProfileConfig` has) |
| `secrets_config` | Secret subsystem | `virtual_keys`, `custom_leak_patterns` |
| `prompt` | Prompt customization | `extra_files` (the only field) |
| `channels.*` | Channel instances | Platform-specific fields (Telegram, Discord, etc.) |
| `a2a` | A2A protocol | `enabled`, `server`, `agents`, `health_check_interval_secs` |
| `acp` | ACP harness | `enabled`, `adapters` (the only two fields) |
| `execution` | Execution engine | `default_timeout_secs` (default 48h), `max_iterations` (default 1000), `prompt_mode`, `progress_push` |
| `agents` | Agent definitions | `defaults`, `list` (array of agent definitions) |
| `bindings` | Route bindings | `match`, `agent_id` (the only two fields) |
| `session` | DM scope — `main` collapses every channel's DM into one agent session | `dm_scope` (`per-peer` default / `main` / `per-channel-peer`), `identity_links` |
| `plugin_marketplaces.*` | Plugin sources | `source`, `type` (`github` or `local`) |
| `stop_hooks` | Stop hooks | `name`, `command`, `timeout_secs` |
| `guardrails` | PII/secret hard-filter switch | `enabled` |
| `stability` | P0 rescue knobs (stall watchdog, failure cap, per-turn timeout) | `stall_timeout_secs`, `consecutive_failure_cap`, `turn_timeout_secs` |
| `fallback_provider` | Provider failover chain | `chain`, `provider` |
| `context_budget` | Mid-run context-window compaction | `enabled`, `token_budget`, `warning_threshold`, `critical_threshold` |
| `strategy` | One-shot strategic planner welded into the cacheable prompt prefix for `/goal`, `/loop`, `/workflow` (defaults on when the section is present) | `enabled`, `planner_model`, `plan_naked_loop`, `plan_team` |
| `moa` | Mixture-of-Agents continuous-advisory presets | `default_preset`, `save_traces`, `presets.*` |
| `team_dispatcher` | Team dispatcher tunables | `max_concurrent`, `lock_ttl_secs`, `task_timeout_secs`, `zombie_ttl_secs`, `max_per_owner`, `default_max_retries`, `retry_backoff_base_secs` |
| `team_broadcast` | Group-chat broadcast storm guards | `max_chain_depth`, `max_fanout_width`, `max_total_activations`, `transcript_token_budget`, `member_run_timeout_secs` |
| `team_messages` | Team thread-escalation nudge | `thread_message_threshold`, `escalation_enabled` |
| `resume` | Boot-scan auto-resume of interrupted runs | `enabled`, `max_age_secs`, `max_attempts`, `max_concurrent` |
| `projects` | Project-selection filesystem scope — the only roots a paired client may traverse | `allowed_roots` (the only field) |
| `tasks_reaper` (alias `task_reaper`) | Background reaper for stale/orphaned tasks | `enabled`, `interval_secs`, `dedup_retention_secs` |
| `desktop` | Desktop daemon-consumer policy | `allow_global_pointer` (default `false`) |
| `gateway` | Gateway server — **foreign-owned**: read by `GatewayConfig::load_default` out of this same file, so `Config` has no `gateway` field by design. Fully live. | `host`, `port`, `max_connections`, `max_connections_per_ip`, `allowed_origins`, `tls`, `rate_limit`, `ping_interval_secs` |

> **Foreign-owned nested key**: `[security.ssrf]` is also live — it is honoured by a
> raw-TOML bridge (`Config::apply_security_ssrf_overrides`) even though
> `ShellSecurityConfig` has no `ssrf` field. Distinct from the top-level `[ssrf]` row above.
>
> **Retired tables — do not write them.** `[agent]` / `[cowork]` (2026-08-17) and
> `[secret_providers]` (2026-09-05) are listed in `src/config/dead_keys.rs` as
> *tolerated-and-retired*: an old file keeps parsing and booting, the loader logs
> them at `info!` telling the operator to delete the line, and they reach no code.
> Also retired: `profiles.*.system_prompt`, `profiles.*.tools`,
> `profiles.*.cache_strategy`, `agents.*.system_prompt`, `[desktop.presence]`,
> `[desktop.mic_level]`, `memory.compound_ingest.replan_on_hash_conflict`,
> `memory.compound_ingest.failure_cooldown_seconds`.
>
> Any section **not** in the table above and **not** in that retired list is neither
> parsed nor tolerated: `Config::load_from_file` warns and `doctor`'s
> `core/config-parse` raises a Warning naming the path as reaching no code.

---

## Operation Protocol

### For Secret/API Key Changes

1. Store secret: `vault_store(action="store", key="ai:openai", secret="sk-...")`
2. Read config: `self_config(action="read_config", config_path="providers.openai")`
3. Preview: `self_config(action="update_config", config_path="providers.openai", config_value={...}, dry_run=true)`
4. Apply + verify: `self_config(action="update_config", ..., dry_run=false, verify=true)` — `verify=true` probes the provider so you confirm the key works immediately.

### For Complex Config Edits (via bash)

**Config path — one spelling, both languages.** `ALEPH_HOME` points *at* the
`.aleph` directory (it is not a parent to join `.aleph` onto); unset it means `~/.aleph`.

- bash: `"${ALEPH_HOME:-$HOME/.aleph}/config.toml"`
- python3: `os.path.join(os.path.expanduser(os.environ.get("ALEPH_HOME", "~/.aleph")), "config.toml")`
  — do **not** shorten this to `open("~/.aleph/…")` or `open("$HOME/.aleph/…")`:
  Python expands neither, so a read raises `FileNotFoundError` and a write
  silently creates a literal `./~/.aleph/` tree in the cwd. And on a host with
  `ALEPH_HOME` set, a hardcoded `~/.aleph` edits a file the daemon never reads.

1. Read current: `bash(cat "${ALEPH_HOME:-$HOME/.aleph}/config.toml")`
2. Edit with python3:
```python
import os, toml
CONFIG = os.path.join(os.path.expanduser(os.environ.get("ALEPH_HOME", "~/.aleph")), "config.toml")
config = toml.load(open(CONFIG))
# modify config
with open(CONFIG, "w") as f:
    toml.dump(config, f)
```
3. Verify: `bash(grep -A10 '\[providers.openai\]' "${ALEPH_HOME:-$HOME/.aleph}/config.toml")`

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

There is exactly **one** secret backend: the built-in encrypted local vault
(`~/.aleph/data/secrets.vault`, AES-256-GCM with the Gateway shared token as
master key). It is **not** configurable — there is no `[secret_providers]`
table and no `[secrets.*]` mapping table. Do not write one — an old config
carrying them still loads (unknown top-level tables are tolerated), but the
table is flagged as retired and resolves nothing.

### Storing and Listing

`vault_store` is the tool surface — see the Vault section above. The
equivalent CLI (useful when the daemon is stopped) is:

```
aleph-server secret init                  # create the vault (idempotent)
aleph-server secret set <NAME>            # prompts for the value (hidden)
aleph-server secret set <NAME> --value …  # non-interactive
aleph-server secret list                  # names only, never values
aleph-server secret delete <NAME>         # requires the server to be stopped
aleph-server secret verify <NAME>         # presence + byte length only
aleph-server secret providers             # prints the one built-in: local / local_vault
```

Secret names accept ASCII letters, digits, `_`, `-`, `.` and `:`, up to 128
characters. Values are never returned by `list`, `verify`, or any RPC.

### Referencing a Secret from Config

Use a `{{secret:NAME}}` placeholder anywhere a config or prompt string is
rendered. It is resolved host-side, immediately before the outbound request,
and the resolved value is fingerprinted for leak detection — it is never
persisted back into `config.toml`.

```toml
[providers.openai]
api_key = "{{secret:ai:openai}}"
```

An unknown name is an error, not an empty string — a missing secret fails the
render rather than silently sending a blank credential.

### `[secrets_config]`

The only secrets section that exists. Both fields are optional.

| Field | Description |
|-------|-------------|
| `virtual_keys` | Alias → real secret name map. `{{secret:<alias>}}` resolves through it; names with no alias pass through unchanged. |
| `custom_leak_patterns` | Extra regex patterns for outbound/inbound leak scanning, additive to the built-ins. Each entry is `{ name, pattern }`; an invalid regex is logged and skipped. |

```toml
[secrets_config.virtual_keys]
"openai" = "OPENAI_API_KEY"

[[secrets_config.custom_leak_patterns]]
name = "Internal API Token"
pattern = "internal-[a-z0-9]{32}"
```

---

## Advanced: Workspace Profiles

Profiles (Anti-Gravity Architecture) bind a class of workspaces to a model and to
cross-workspace memory-recall behaviour. They are static templates; workspaces are
runtime instances. `ProfileConfig` has exactly **two** fields — everything else a
profile used to accept was retired because it reached no code.

```toml
[profiles.coding]
model = "claude-sonnet-4"

[profiles.coding.smart_recall]
enabled = true
score_threshold = 0.60
min_primary_results = 2
max_cross_results = 3

[profiles.creative]
model = "gemini-2.5-flash"
```

| Field | Description |
|-------|-------------|
| `model` | Bound AI model, resolved by `agent_resolver::resolve_one` (overrides the general default) |
| `smart_recall` | Cross-workspace memory recall, plumbed into the `memory_search` per-agent handle: `enabled` (default `true`), `score_threshold` (default `0.60`), `min_primary_results` (default `2`), `max_cross_results` (default `3`) |

> **Do not write these into a profile.** `tools`, `system_prompt` and
> `cache_strategy` are tolerated-and-retired (`dead_keys.rs` logs them at `info!`);
> `description`, `temperature`, `max_tokens` and `history_limit` were retired in the
> 2026-08-17 wire audit (config-004) and are not even tolerated — they are **reported
> dead**. The live equivalents live elsewhere: the tool gate is
> `AgentInstanceConfig.tool_whitelist` (sourced from `agent.skills`), the persona
> overlay is the workspace's `AGENTS.md`, and prompt caching is decided by the
> protocol adapters with no dial at all.

---

## Advanced: Slash-Command Rules (`[[rules]]`)

`[[rules]]` defines **custom slash commands**. Nothing else — keyword rules
(2026-09-05) are retired: they were never wired to anything, and wiring them
would be intent routing by rule engine, which R7/P8 forbids.

```toml
[[rules]]
regex = "^/draw\\s+"        # MUST start with ^/ — this is what registration keys off
provider = "gemini"         # required by validation (see below), read by nothing
system_prompt = "Draw a picture based on the prompt"
```

A rule whose `regex` starts with `^/` and that is not `is_builtin` is registered
by `register_custom_commands` as a tool named after the command, with
`system_prompt` as both its description (truncated to 100 chars) and its routing
prompt. A rule whose regex lacks the prefix is registered nowhere and does
nothing.

What happens to a non-`^/` rule now depends on how it arrives:

| Path | Behaviour |
|------|-----------|
| Written via `routing_rules.create` / `routing_rules.update` | **Refused.** Both handlers call `reject_unregistrable`, which rejects a non-`^/` regex *and* any `rule_type` label other than `"command"` (a mislabelled `^/` rule would be listed to the Panel as "KEYWORD" — a wrong label reads as a fact). |
| Already on disk (hand-edited `config.toml`) | **Warned, not rejected.** `Config::validate` names every inert rule at `warn!` with its 1-based index and regex, then returns `Ok` — the daemon still boots. Editing `config.toml` by `bash` bypasses the write-boundary refusal above, so this warning is the only signal you get. |
| Panel rule editor | The rule-type selector is gone; the form writes `rule_type: None` and the type is derived from the regex. |

> **The two predicates are not the same set.** *Registration* keys off the
> regex (`RoutingRuleConfig::is_registered_command` — `^/` prefix). The
> `provider` **requirement** in `Config::validate` still keys off the
> operator's `rule_type` *label* (`is_command_rule` → `get_rule_type`, which
> falls back to the regex only when the label is absent). So `provider` is
> genuinely required — the file fails to load without it — but of a *different*
> set than the one that gets registered: `rule_type = "keyword"` on a `^/`
> regex is registered yet never provider-checked, and `rule_type = "command"`
> on a plain regex is provider-checked yet reaches no code. That split is
> deliberate: deriving the requirement from the regex would hard-fail configs
> that boot today.

`provider` is required and then read by nothing — no routing pass turns a
rule's `provider` into a model choice. `preferred_model`, `strip_prefix`,
`intent_type` and `icon` parse cleanly and are dropped. Source of truth:
`src/config/types/routing.rs` module doc.

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
