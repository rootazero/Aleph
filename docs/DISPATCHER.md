# Dispatcher Layer (Aether Cortex)

The Dispatcher Layer provides intelligent tool routing with confidence-based confirmation.

## Architecture Overview

```
User Input
     |
+---------------------+
|   Dispatcher Layer  |
|                     |
|  +---------------+  |
|  | ToolRegistry  |  |  <- Aggregates Native/MCP/Skills/Custom
|  +-------+-------+  |
|          |          |
|  +---------------+  |
|  | Multi-Layer   |  |
|  | Router        |  |  <- L1 -> L2 -> L3 cascade
|  +-------+-------+  |
|          |          |
|  +---------------+  |
|  | Confirmation  |  |  <- If confidence < threshold
|  +---------------+  |
+----------+----------+
           |
   Execution Layer
```

---

## Multi-Layer Routing

| Layer | Method | Latency | Confidence | Use Case |
|-------|--------|---------|------------|----------|
| L1 | Regex pattern match | <10ms | 1.0 | Explicit slash commands (`/search`, `/translate`) |
| L2 | Semantic keyword match | 200-500ms | 0.7 | Natural language with keywords ("search for...", "translate this") |
| L3 | LLM inference | >1s | 0.5-0.9 | Ambiguous intent, pronoun resolution, complex queries |
| Default | Fallback | 0ms | 0.0 | General chat when no tool matches |

### Routing Cascade

- L1 tries first -> if match (confidence >= 0.9), execute
- L2 tries if L1 fails -> if match (confidence >= 0.7), execute
- L3 tries if L2 fails or confidence too low -> AI decides tool + params
- Default provider if all layers fail

---

## Tool Sources (Flat Namespace Mode)

| Source | Description | Example |
|--------|-------------|---------|
| `Builtin` | System builtin commands (3 only) | `/search`, `/video`, `/chat` |
| `Native` | Built-in capabilities | Search, Video transcript |
| `Mcp` | MCP server tools (flat) | `/git`, `/filesystem` |
| `Skill` | Claude Agent Skills (flat) | `/refine-text`, `/code-review` |
| `Custom` | User-defined rules | `[[rules]]` in config.toml |

### Flat Namespace Design

All tools are registered as root-level commands. Users invoke tools directly by name:
- `/git status` - Correct (MCP tool invoked directly)
- `/refine-text` - Correct (Skill invoked directly)
- `/mcp git status` - NOT supported (namespace prefix removed)
- `/skill refine-text` - NOT supported (namespace prefix removed)

Tool source is indicated via UI badges (System, MCP, Skill, Custom), not command prefixes.

---

## Single Source of Truth (BUILTIN_COMMANDS)

The 3 builtin commands are defined in `dispatcher/builtin_defs.rs`:

```rust
pub const BUILTIN_COMMANDS: &[BuiltinCommandDef] = &[
    BuiltinCommandDef { name: "search", ... },
    BuiltinCommandDef { name: "video", ... },
    BuiltinCommandDef { name: "chat", ... },
];
```

Used by:
- `ToolRegistry.register_builtin_tools()` - for tool metadata
- `get_builtin_routing_rules()` - for routing config
- Config module - for default rules

---

## Confirmation Flow

- Tools with `confidence < confirmation_threshold` trigger user confirmation
- Halo shows tool preview (name, icon, parameters)
- User can Execute, Edit parameters, or Cancel
- Cancel falls back to GeneralChat

---

## Event System (Tool Changes)

When tools change (MCP connect/disconnect, skill install/uninstall):

```
Rust: refresh_tool_registry()
    |
Rust: event_handler.on_tools_changed(tool_count)
    |
Swift: EventHandler.onToolsChanged() posts .toolsDidChange notification
    |
Swift: CommandCompletionManager auto-refreshes command list
```

---

## L3 Agent (Multi-step Planning)

The L3 Agent enables intelligent multi-step task planning where complex requests are decomposed into sequential tool invocations.

### Planning Architecture

```
User Input
     |
+-------------------------+
|   QuickHeuristics       |  <- Fast detection (<10ms)
|   (is_likely_multi_step)|     Detects action verbs + connectors
+-----------+-------------+
            | (if multi-step likely)
+-------------------------+
|   L3TaskPlanner         |  <- LLM-based planning
|   (analyze_and_plan)    |     Generates TaskPlan with steps
+-----------+-------------+
            |
+-------------------------+
|   PlanningResult        |
|   - Plan(TaskPlan)      |  -> Multi-step execution
|   - SingleTool          |  -> Direct tool execution
|   - GeneralChat         |  -> AI conversation
+-----------+-------------+
            | (if Plan)
+-------------------------+
|   PlanConfirmationView  |  <- User confirms plan
|   (Swift UI)            |     Shows steps + safety warnings
+-----------+-------------+
            |
+-------------------------+
|   PlanExecutor          |  <- Sequential execution
|   (execute_plan)        |     Resolves $prev references
+-----------+-------------+
            |
+-------------------------+
|   PlanProgressView      |  <- Live progress display
|   (Swift UI)            |     Shows step status + results
+-------------------------+
```

### $prev Parameter Chaining

Steps in a plan can reference the output of the previous step using `$prev`:

```json
{
  "steps": [
    {"tool": "search", "params": {"query": "AI news"}},
    {"tool": "summarize", "params": {"text": "$prev"}},
    {"tool": "translate", "params": {"text": "$prev", "target": "Chinese"}}
  ]
}
```

### Tool Safety Levels

| Level | Description | Confirmation | Rollback |
|-------|-------------|--------------|----------|
| `ReadOnly` | No state changes (search, read) | No | N/A |
| `Reversible` | Can be undone (copy file, create) | If low confidence | Yes |
| `IrreversibleLowRisk` | Minor permanent changes | Yes | No |
| `IrreversibleHighRisk` | Major permanent changes (delete) | Always | No |

---

## Configuration

See [CONFIGURATION.md](./CONFIGURATION.md#dispatcher) for full configuration options.

```toml
[dispatcher]
enabled = true
l3_enabled = true
l3_timeout_ms = 5000
confirmation_enabled = true
confirmation_threshold = 0.7
confirmation_timeout_ms = 30000

[dispatcher.agent]
enabled = true
max_steps = 10
step_timeout_ms = 30000
enable_rollback = true
plan_confirmation_required = true
allow_irreversible_without_confirmation = false
heuristics_threshold = 2
```

---

## UniFFI Interface

```swift
// List all available tools
let tools = core.listTools()

// Filter by source type
let mcpTools = core.listToolsBySource(sourceType: .mcp)

// Search tools by query
let matches = core.searchTools(query: "git")

// Refresh registry (after MCP server changes)
try core.refreshTools()
```

---

## Code Locations

| Component | Location |
|-----------|----------|
| Dispatcher module | `Aether/core/src/dispatcher/` |
| Builtin definitions | `dispatcher/builtin_defs.rs` |
| Tool Registry | `dispatcher/registry.rs` |
| L3 Router | `dispatcher/l3_router.rs` |
| Prompt Builder | `dispatcher/prompt_builder.rs` |
| Confirmation | `dispatcher/confirmation.rs` |
| Integration | `dispatcher/integration.rs` |
| L3 Agent Planning | `routing/planner.rs`, `routing/executor.rs` |
| Quick Heuristics | `routing/heuristics.rs` |
| Tool Handlers | `tools/handler.rs`, `tools/params/` |
| Agent Loop | `agent/executor.rs` |
| Rollback Support | `routing/rollback.rs` |
| Swift event handler | `Sources/EventHandler.swift` |
| Swift notifications | `Sources/Notifications.swift` |
| Swift Plan UI | `Components/PlanConfirmationView.swift`, `Components/PlanProgressView.swift` |
| Command completion | `Sources/Utils/CommandCompletionManager.swift` |

---

**Last Updated**: 2026-01-11
