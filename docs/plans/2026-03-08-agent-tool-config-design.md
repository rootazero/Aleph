# Per-Agent Tool Configuration Design

## Goal

Allow users to configure which tools each agent can use, with group-level and individual-tool-level granularity, manageable via both TOML config and Panel UI.

## Approach

Extend existing `AgentDefinition.skills` (whitelist) with a new `skills_blacklist` field. Panel UI renders tools in functional groups with dual-layer toggles. Backward compatible — no breaking changes.

## Tool Groups

23 builtin tools organized into 6 groups (defined as code constants, not stored in TOML):

| Group ID | Display Name | Tools |
|----------|-------------|-------|
| `search_web` | 搜索与网络 | `search`, `web_fetch`, `youtube` |
| `file_code` | 文件与代码 | `file_ops`, `bash`, `code_exec`, `pdf_generate` |
| `memory_knowledge` | 记忆与知识 | `memory_search`, `memory_browse`, `read_skill`, `list_skills` |
| `content_gen` | 内容生成 | `generate_image` |
| `system_config` | 系统与配置 | `desktop`, `config_read`, `config_update` |
| `agent_mgmt` | Agent 管理 | `agent_create`, `agent_switch`, `agent_list`, `agent_delete`, `sessions_list`, `sessions_send`, `subagent_spawn`, `subagent_steer`, `subagent_kill`, `escalate_task` |

## Configuration Model

### TOML Config

```toml
[agents.defaults]
skills_blacklist = []              # Global default blacklist

[[agents.list]]
id = "trader"
name = "交易助手"
skills = ["*"]                     # Whitelist (default: ["*"])
skills_blacklist = ["bash", "code_exec"]  # Blacklist (new field)
```

### Semantic Rules

- `skills` unset → defaults to `["*"]` (all allowed)
- `skills = ["search", "file_ops"]` → only these allowed
- `skills_blacklist` → always denied regardless of whitelist
- Blacklist priority > whitelist (existing logic in `is_tool_allowed()`)

### Resolution Cascade

```
skills:           agent.skills > defaults.skills > ["*"]
skills_blacklist:  agent.skills_blacklist > defaults.skills_blacklist > []
```

## Data Flow

```
aleph.toml                    ResolvedAgent                   AgentInstanceConfig
─────────                     ─────────────                   ───────────────────
skills           ──resolve──> skills          ──map──>       tool_whitelist
skills_blacklist ──resolve──> skills_blacklist ──map──>      tool_blacklist
                                                                  │
                                                    is_tool_allowed()
                                                    (no changes needed)
```

## RPC Changes

### `agents.update` — Patch Extension

`AgentPatch` gains `skills_blacklist: Option<Vec<String>>`.

### `agents.tools_schema` — New Endpoint

Returns tool group metadata for Panel rendering:

```json
{
  "groups": [
    {
      "id": "search_web",
      "name": "搜索与网络",
      "tools": [
        { "name": "search", "display_name": "Internet Search", "description": "..." },
        { "name": "web_fetch", "display_name": "Web Fetch", "description": "..." }
      ]
    }
  ]
}
```

## Panel UI

### Location

Settings → Agents → [Agent] → **Tools** tab (alongside existing Behavior tab).

### Layout

```
┌─────────────────────────────────────────────────┐
│  Tools Configuration                             │
│  Configure which tools this agent can use        │
├─────────────────────────────────────────────────┤
│                                                  │
│  ┌─ 搜索与网络 ──────────────── [全组开关] ─┐   │
│  │  ☑ search       Internet Search           │   │
│  │  ☑ web_fetch    Web Fetch                 │   │
│  │  ☑ youtube      YouTube Transcript        │   │
│  └───────────────────────────────────────────┘   │
│                                                  │
│  ┌─ 文件与代码 ──────────────── [全组开关] ─┐   │
│  │  ☑ file_ops     File Operations           │   │
│  │  ☐ bash         Shell Commands            │   │
│  │  ☐ code_exec    Code Execution            │   │
│  │  ☑ pdf_generate PDF Generation            │   │
│  └───────────────────────────────────────────┘   │
│                                                  │
│  ... other groups ...                            │
│                                                  │
│                              [Reset] [Save]      │
└─────────────────────────────────────────────────┘
```

### Interaction

- **Group toggle**: Flips all tools in the group
- **Group state**: All on = on, all off = off, mixed = indeterminate
- **Individual toggle**: Independent control, updates group state
- **Reset**: Restores `skills = ["*"]`, `skills_blacklist = []`
- **Save**: Computes minimal skills/skills_blacklist combination

### Save Logic (UI → TOML)

Panel maintains a `Set<String>` of enabled tool names. On save:

1. All enabled → `skills = ["*"]`, `skills_blacklist = []`
2. Few disabled → `skills = ["*"]`, `skills_blacklist = [disabled tools]`
3. Few enabled → `skills = [enabled tools]`, `skills_blacklist = []`
4. Threshold: use strategy 2 when disabled_count ≤ enabled_count, else strategy 3

## Backend Changes

| File | Change |
|------|--------|
| `config/types/agents_def.rs` | Add `skills_blacklist: Option<Vec<String>>` to `AgentDefinition` and `AgentDefaults` |
| `config/agent_resolver.rs` | Add `skills_blacklist: Vec<String>` to `ResolvedAgent`, resolve cascade |
| `gateway/agent_instance.rs` | Map `skills_blacklist` → `tool_blacklist` in `from_resolved()` |
| `gateway/handlers/agents.rs` | Add `skills_blacklist` to `AgentPatch`; add `agents.tools_schema` handler |
| `executor/builtin_registry/groups.rs` | **New file**: `ToolGroup` struct + 6 group constants + `all_groups()` |
| Panel `views/settings/agents/` | **New file**: `tools_tab.rs` with group/tool toggle UI |

### No Changes Needed

- `is_tool_allowed()` — already supports blacklist priority + `"*"` wildcard + glob prefix
- `ToolFilter` (thinker) — upstream filtering already handled
- Existing `skills` field semantics — fully backward compatible
