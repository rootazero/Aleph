# Model-Perceivable Ecosystem Design

> Date: 2026-04-06
> Status: Approved
> Scope: Agent Catalog + MCP Tool Discovery (Priority Group A)

## Problem

Aleph's prompt pipeline has 30+ layers for capability injection, but the LLM lacks awareness of two critical extension dimensions:

1. **Sub-agents**: The primary agent doesn't know what sub-agents exist, what they do, or when to delegate to them. Only sub-agents receive role headers (via AgentRoleLayer).
2. **MCP tools**: McpInstructionsLayer injects server usage instructions but doesn't list the actual tools each server provides. The model can't discover MCP capabilities.

Claude Code solves this by injecting skills lists, agent lists, MCP instructions, and session-specific guidance — making the model "perceive" its full extension ecosystem. Aleph needs equivalent capability, adapted to its Rust architecture and two-stage discovery pattern.

## Design Approach

**Lightweight Catalog + On-Demand Deep-Dive** — consistent with Aleph's existing patterns:
- native `tool_use` schemas + `tool_search` / `get_tool_schema` (progressive tool disclosure)
- SkillInstructionsLayer + skill_read (deferred loading)

Inject a compact index via prompt layers; provide tools for detailed queries.

## Part 1: AgentDef Extension

### Changes to `src/agents/types.rs`

Add two fields to `AgentDef`:

```rust
pub struct AgentDef {
    pub id: String,
    pub description: String,         // One-line description for catalog index
    pub when_to_use: Option<String>,  // Usage trigger hint for the model
    pub mode: AgentMode,
    pub prompt_sections: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub denied_tools: Vec<String>,
    pub max_iterations: Option<u32>,
    pub token_budget: Option<u32>,
    pub model_hint: Option<String>,
    pub context_mode: ContextMode,
}
```

Builder methods:
- `with_description(impl Into<String>) -> Self`
- `with_when_to_use(impl Into<String>) -> Self`

### Changes to `src/agents/registry.rs`

Update `builtin_agents()` with descriptions:

| Agent | description | when_to_use |
|-------|-------------|-------------|
| main | Primary agent that responds directly to user | (None — not in catalog) |
| explore | Read-only codebase exploration specialist | When you need to search, read, or understand code without modifying anything |
| coder | Code writing specialist with file operations | When you need to write, edit, or create code files |
| researcher | Web and document research specialist | When you need to search the web, fetch URLs, or gather external information |
| default | General-purpose sub-agent | When no specialized agent fits the task |
| plan | Read-only planning and analysis specialist | When you need to analyze requirements, design architecture, or create implementation plans |
| verify | Adversarial verification specialist | When you need to independently verify that work was done correctly |

### Compatibility

- `AgentDef::new()` sets `description` to empty string, `when_to_use` to `None`
- All existing code compiles without changes
- User-defined agents via TOML can optionally set these fields

## Part 2: AgentCatalogLayer

### Layer Properties

| Property | Value |
|----------|-------|
| Name | `agent_catalog` |
| Priority | 505 |
| Stability | Stable |
| Modes | Full only |
| Paths | Basic, Soul, Context, Cached |

### New File: `src/thinker/layers/agent_catalog.rs`

### Data Types

```rust
// In src/thinker/prompt_layer.rs
pub struct AgentCatalogEntry {
    pub id: String,
    pub description: String,
    pub when_to_use: Option<String>,
}
```

### Data Flow

```
PromptConfig.available_agents: Option<Vec<AgentCatalogEntry>>
  → LayerInput reads from config
  → AgentCatalogLayer.inject() generates XML index
```

### Prompt Output Format

```markdown
## Available Agents

You can delegate tasks to specialized sub-agents using the `delegate` tool.
Use `agent_info(agent_id)` to get detailed capabilities before delegating.

<available_agents>
  <agent>
    <id>explore</id>
    <description>Read-only codebase exploration specialist</description>
    <when>When you need to search, read, or understand code without modifying anything</when>
  </agent>
  <agent>
    <id>coder</id>
    <description>Code writing specialist with file operations</description>
    <when>When you need to write, edit, or create code files</when>
  </agent>
  ...
</available_agents>
```

### Filtering Rules

- Only SubAgent mode agents are listed (Primary excluded)
- Only agents with non-empty `description` are listed
- Sorted by id for deterministic output

## Part 3: McpToolIndexLayer

> **⚠️ Status (2026-07-17): REMOVED / superseded — this section is historical.**
> `McpToolIndexLayer` (`mcp_tool_index.rs`) and the `mcp_tool_schema` tool
> (`builtin_tools/mcp_discover/`) described below were **removed as dead code on
> 2026-05-31** (see `src/thinker/prompt_pipeline.rs`); no `mcp_tool_index.rs`
> exists. MCP **tools** reach the model through normal dynamic tool-table
> registration (`mcp::tool_bridge` → `register_mcp_tools`), optionally deferred
> behind `tool_search` when `[tools] defer_mcp_tools` is on.
>
> MCP **resources / prompts** are instead surfaced through **discovery tools**
> (2026-07-17): `mcp_list_resources` / `mcp_list_prompts`
> (`src/builtin_tools/mcp_resource.rs` / `src/builtin_tools/mcp_prompt.rs`,
> colocated with their read twins) enumerate each server's resources /
> prompts as server-qualified identifiers, capability-gated alongside
> `mcp_read_resource` / `mcp_get_prompt` in `mcp::tool_bridge`. This replaces the
> prompt-index approach with a model-initiated list→read flow (R7/R10 static
> partition, no prompt-layer). See FEATURE_LOCATOR §3.9.
>
> **Read-path defense (2026-07-17 follow-up)**: a non-blocking cat-guard
> (`src/tools/scoped/cat_guard.rs`, wired at the single dispatch chokepoint
> `dispatch.rs::execute_inner`) appends a `<system-reminder>` steering a raw
> `file_read` / shell `cat` of an installed **skill** (or plugin-shipped skill)
> file toward `skill_read`. Skill-only by design — MCP resources are server URIs
> with no on-disk root, so a `cat` of one is not a filesystem path (the MCP
> surface is covered by the discovery tools above). Identifier note: the
> `mcp_list_resources` id carries a **doubled** server prefix that is
> load-bearing (the read path strips two symmetric layers), so it is presented
> to the model as an opaque pass-verbatim token. See FEATURE_LOCATOR
> §3.9 / §3.10 / §3.11.

### Layer Properties

| Property | Value |
|----------|-------|
| Name | `mcp_tool_index` |
| Priority | 1065 |
| Stability | Dynamic |
| Modes | Full only |
| Paths | Basic, Hydration, Soul, Context, Cached |

### New File: `src/thinker/layers/mcp_tool_index.rs`

### Data Types

```rust
// In src/thinker/prompt_layer.rs
pub struct McpToolIndexEntry {
    pub server_name: String,
    pub tool_name: String,
    pub description: String,
}
```

### Data Flow

```
McpClient.list_tools()           // Already cached internally
  → Vec<McpTool>
  → Group by server_name prefix
  → Vec<McpToolIndexEntry>       // Strip input_schema
  → LayerInput.mcp_tool_index: Option<&[McpToolIndexEntry]>
  → McpToolIndexLayer.inject()
```

### Prompt Output Format

```markdown
## MCP Server Tools

The following tools are provided by connected MCP servers.
Use `mcp_tool_schema(server, tool)` to get full parameter schema before calling.

### github
- github:create_issue — Create a new issue in a repository
- github:list_pulls — List pull requests with filters
- github:search_code — Search code across repositories

### slack
- slack:send_message — Send a message to a channel
- slack:list_channels — List available channels
```

### Relationship with McpInstructionsLayer

- McpInstructionsLayer (1060): HOW to use a server (static instructions)
- McpToolIndexLayer (1065): WHAT tools a server provides (dynamic index)

Complementary, not overlapping. Both inject when their respective data is present.

### Empty State

When no MCP servers are connected or all servers have zero tools, the layer produces no output (no empty section header).

## Part 4: On-Demand Tools

### 4a. `agent_info` Tool

**Location**: `src/builtin_tools/agent_manage/info.rs`

**Parameters**:
```json
{
  "agent_id": { "type": "string", "description": "Agent ID to look up" }
}
```

**Returns** (JSON):
```json
{
  "id": "explore",
  "description": "Read-only codebase exploration specialist",
  "when_to_use": "When you need to search, read, or understand code without modifying anything",
  "mode": "SubAgent",
  "allowed_tools": ["glob", "grep", "read_file", "web_fetch", "search"],
  "denied_tools": ["write_file", "edit_file", "bash"],
  "max_iterations": 20,
  "context_mode": "Fresh",
  "model_hint": null,
  "token_budget": null
}
```

**Error**: `{"error": "Agent 'xyz' not found. Available agents: explore, coder, researcher, default, plan, verify"}`

**Properties**: Read-only, no side effects, no user confirmation required.

### 4b. `mcp_tool_schema` Tool

**Location**: `src/builtin_tools/mcp_discover/mod.rs`

**Parameters**:
```json
{
  "tool_name": { "type": "string", "description": "Full tool name (e.g., 'github:create_issue')" }
}
```

**Returns** (JSON):
```json
{
  "tool_name": "github:create_issue",
  "server_name": "github",
  "description": "Create a new issue in a repository",
  "input_schema": { "type": "object", "properties": { "..." : "..." } },
  "requires_confirmation": false
}
```

**Error**: `{"error": "MCP tool 'xyz' not found. Use the MCP Server Tools section in system prompt to see available tools."}`

**Properties**: Read-only, no side effects, no user confirmation required.

## Part 5: Assembly & Prompt Cache Strategy

### Prompt Cache Boundary

```
Stable Zone (cached):
  50   SoulLayer
  55   AgentRoleLayer
  75   ProfileLayer
  ...
  505  AgentCatalogLayer          ← added by this design (Stable)
  ...
  1050 SkillInstructionsLayer
  ─── cache boundary ───
Dynamic Zone (per-request):
  1060 McpInstructionsLayer
  ...
```

> Illustrative, not authoritative — `PromptPipeline::default_layers()` is the
> only current list, and `aleph-server prompt-size` prints it. (`ToolsLayer` /
> `HydratedToolsLayer` / `McpToolIndexLayer`, named in the original sketch, have
> since been deleted.)

AgentCatalogLayer is Stable — agent list doesn't change mid-session. Participates in prompt cache, no extra cost per request.

McpToolIndexLayer is Dynamic — MCP servers may connect/disconnect. Sits after cache boundary alongside existing McpInstructionsLayer.

### Assembly Points

**AgentCatalogLayer data** (at startup / agent registration):
```
AgentRegistry.list_subagents()
  → filter(|a| !a.description.is_empty())
  → map to AgentCatalogEntry
  → PromptConfig.available_agents = Some(entries)
```

**McpToolIndexLayer data** (per prompt assembly):
```
McpClient.list_tools()               // has internal cache
  → group by server_name prefix
  → map to McpToolIndexEntry
  → LayerInput.with_mcp_tool_index(&entries)
```

## File Change Summary

### New Files (4)

| File | Type | Description |
|------|------|-------------|
| `src/thinker/layers/agent_catalog.rs` | Layer | AgentCatalogLayer implementation + tests |
| `src/thinker/layers/mcp_tool_index.rs` | Layer | McpToolIndexLayer implementation + tests |
| `src/builtin_tools/agent_manage/info.rs` | Tool | agent_info tool implementation |
| `src/builtin_tools/mcp_discover/mod.rs` | Tool | mcp_tool_schema tool implementation |

### Modified Files (7)

| File | Change |
|------|--------|
| `src/agents/types.rs` | Add description, when_to_use fields + builder methods |
| `src/agents/registry.rs` | Update builtin_agents() with descriptions |
| `src/thinker/layers/mod.rs` | Register new layers |
| `src/thinker/prompt_layer.rs` | Add AgentCatalogEntry, McpToolIndexEntry, LayerInput fields |
| `src/thinker/prompt_pipeline.rs` | Add new layers to default_layers() |
| `src/thinker/prompt_builder/mod.rs` | Add available_agents to PromptConfig |
| `src/builtin_tools/agent_manage/mod.rs` | Register info tool |

### Zero Breaking Changes

- All new AgentDef fields have defaults (empty string / None)
- All new PromptConfig fields are Option
- All new LayerInput fields are Option with None default
- Existing tests pass without modification

## Testing Strategy

Each component gets unit tests following existing patterns:

- **AgentCatalogLayer**: test XML generation, filtering (SubAgent only, non-empty description), empty state, all assembly paths
- **McpToolIndexLayer**: test markdown generation, grouping by server, empty state, Dynamic stability
- **agent_info tool**: test found/not-found, JSON serialization
- **mcp_tool_schema tool**: test found/not-found, schema passthrough

Integration test: build full prompt pipeline with both layers active, verify output contains agent catalog and MCP tool index sections.
