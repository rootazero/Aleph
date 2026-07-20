# Tools Visibility — Catalog & Effective RPC

**Date**: 2026-03-27
**Status**: Approved
**Scope**: `tools.catalog` + `tools.effective` RPC handlers (using existing `AgentDef.denied_tools`)

## Background

Aleph currently has `commands.list` which returns all tools as a flat hierarchical tree. There is no way to:
- See tools grouped by source (builtin/mcp/skill/plugin)
- Know what tools a specific agent can actually use (after deny-list filtering)
- Query effective tools without session context

OpenClaw ships `tools.catalog` (static full list) and `tools.effective` (session-scoped, filtered). Aleph's version is simpler — no profile system (R8 LLM sovereignty), no channel filtering (YAGNI) — just agent-level deny list filtering.

## Design Decisions

1. **No profiles** — All registered tools available by default. Fits R8 (LLM decides what to use).
2. **Reuse existing `AgentDef.denied_tools`** — `AgentDef` already has `denied_tools: Vec<String>` and `is_tool_allowed()` method that respects both allow and deny lists. No new field needed.
3. **No channel filtering** — Deferred. LLM is smart enough not to use irrelevant tools.
4. **No Panel UI** — Backend RPC only. UI is a separate iteration.

## RPC Methods

### `tools.catalog`

Returns all registered tools grouped by source. No filtering applied.

**Request:**
```json
{ "method": "tools.catalog", "params": {} }
```

No parameters — returns the global tool catalog regardless of agent.

**Response:**
```json
{
  "total": 42,
  "groups": [
    {
      "id": "builtin",
      "label": "Built-in",
      "source": "builtin",
      "tools": [
        {
          "name": "search",
          "description": "Search the web",
          "source": "builtin",
          "source_id": null
        }
      ]
    },
    {
      "id": "mcp:browser",
      "label": "browser",
      "source": "mcp",
      "source_id": "browser",
      "tools": [...]
    },
    {
      "id": "skill",
      "label": "Skills",
      "source": "skill",
      "tools": [...]
    },
    {
      "id": "plugin:cli-anything",
      "label": "cli-anything",
      "source": "plugin",
      "source_id": "cli-anything",
      "tools": [...]
    }
  ]
}
```

### `tools.effective`

Same format as catalog, but filtered through the agent's `is_tool_allowed()` method (respects both `allowed_tools` and `denied_tools`).

**Request:**
```json
{ "method": "tools.effective", "params": { "agent_id": "iris" } }
```

`agent_id` is optional — defaults to the default agent.

**Response:** Same structure, minus tools rejected by `agent.is_tool_allowed(tool_name)`. `total` reflects the filtered count.

### Shared Response Type

Both methods return the same `ToolsListResult`:

```rust
#[derive(Debug, Serialize)]
pub struct ToolsListResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,  // present in effective, absent in catalog
    pub total: usize,
    pub groups: Vec<ToolGroup>,
}

#[derive(Debug, Serialize)]
pub struct ToolGroup {
    pub id: String,           // "builtin", "mcp:server_name", "skill", "plugin:name"
    pub label: String,        // Display name
    pub source: String,       // "builtin" | "mcp" | "skill" | "plugin" | "custom"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,  // MCP server name, plugin ID, etc.
    pub tools: Vec<ToolEntry>,
}

#[derive(Debug, Serialize)]
pub struct ToolEntry {
    pub name: String,
    pub description: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
}
```

## Agent Tool Filtering

### Existing Infrastructure

`AgentDef` (at `src/agents/types.rs`) already has:
- `denied_tools: Vec<String>` — tool names to exclude
- `allowed_tools: Vec<String>` — tool names to allow (`"*"` = all)
- `is_tool_allowed(&self, tool_name: &str) -> bool` — respects both allow and deny lists

**No new fields needed.** The `tools.effective` handler simply calls the existing `is_tool_allowed()` method.

### Filtering Logic

```rust
fn filter_effective(tools: Vec<UnifiedTool>, agent: &AgentDef) -> Vec<UnifiedTool> {
    tools.into_iter()
        .filter(|t| agent.is_tool_allowed(&t.name))
        .collect()
}
```

Applied only in `tools.effective`, not in `tools.catalog`.

### Configuration

Users configure allow/deny lists via:
- Agent YAML config: `denied_tools: [snapshot, screen_capture]`
- Conversation (R9): LLM calls agent management tool to update lists

## Grouping Logic

Tools are grouped by their `ToolSource` (rich enum with embedded data). Extract source type and ID via pattern matching:

```rust
let (source_str, source_id) = match &tool.source {
    ToolSource::Native => ("native", None),
    ToolSource::Builtin => ("builtin", None),
    ToolSource::Mcp { server } => ("mcp", Some(server.clone())),
    ToolSource::Skill { id } => ("skill", Some(id.clone())),
    ToolSource::Plugin { plugin_id } => ("plugin", Some(plugin_id.clone())),
    ToolSource::Custom => ("custom", None),
};
```

Note: `ToolSource` is the actual enum on `UnifiedTool.source` — it carries source_id inline.

| ToolSource variant | Group ID | Group Label |
|--------------------|----------|-------------|
| `Native` | `"native"` | `"Native"` |
| `Builtin` | `"builtin"` | `"Built-in"` |
| `Mcp { server }` | `"mcp:{server}"` | `"{server}"` |
| `Skill { id }` | `"skill:{id}"` | `"{id}"` |
| `Plugin { plugin_id }` | `"plugin:{plugin_id}"` | `"{plugin_id}"` |
| `Custom` | `"custom"` | `"Custom"` |

MCP, Skill, and Plugin tools are sub-grouped by their source identifier. Native, Builtin, and Custom are each a single group.

## Implementation Location

**New file:** `src/gateway/handlers/tools_visibility.rs`

Two handler functions registered in `HandlerRegistry`:
- `tools.catalog` → `handle_catalog`
- `tools.effective` → `handle_effective`

Both handlers need access to:
- `ToolRegistry` (from dispatcher) — to list all tools via `UnifiedTool` entries
- Agent definitions (from `agents::registry::AgentRegistry`) — to look up `is_tool_allowed()` for effective filtering

**Handler wiring pattern:** Follow the existing stub-then-wire pattern used by `commands.list`:
1. Register stubs in `HandlerRegistry::new()` pointing to the handler functions
2. At Gateway startup, wire with captured `Arc` references to `ToolRegistry` and agent data
3. Handlers receive these via `HandlerContext` closures

## Error Handling

| Scenario | Error |
|----------|-------|
| Agent not found | `{ "error": "Agent 'xxx' not found" }` |
| ToolRegistry not available | `{ "error": "Tool registry not available" }` |

## Not In Scope

- Panel UI ("Available Right Now" section) — separate iteration
- Channel-based filtering — deferred
- Profile presets (minimal/coding/messaging/full) — conflicts with R8
- Tool ordering/priority
- Per-tool enable/disable toggle in UI

## Acceptance Criteria

1. `tools.catalog` RPC returns all registered tools grouped by source (no params needed)
2. `tools.effective` RPC returns tools filtered by `AgentDef.is_tool_allowed()`
3. `agent_id` parameter on `tools.effective` is optional (defaults to default agent)
4. Uses existing `AgentDef.denied_tools` + `allowed_tools` — no new config fields
5. Groups correctly separate native/builtin/mcp/skill/plugin/custom tools
6. MCP, Skill, Plugin tools sub-grouped by source identifier
7. Existing tests unaffected
