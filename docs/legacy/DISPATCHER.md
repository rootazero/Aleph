# Dispatcher Layer (Aleph Cortex)

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

## Tool Registry

Tools are registered and managed through the ToolRegistry in `dispatcher/registry.rs`:

- **Native tools**: Registered via `executor/builtin_registry.rs`
- **MCP tools**: Dynamic registration from MCP servers
- **Skills**: Registered from skill definitions

The registry provides unified access to all tool sources through a flat namespace.

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

## Task Orchestration (Cowork)

Cowork is Aleph's multi-task orchestration system that decomposes complex user requests into DAG-structured task graphs and executes them with parallel scheduling.

### Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                      Dispatcher (CoworkEngine)                   │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐   │
│  │  TaskPlanner │  │ DAGScheduler │  │  ExecutorRegistry    │   │
│  │  (LLM-based) │  │  (topo sort) │  │  (extensible)        │   │
│  └──────────────┘  └──────────────┘  └──────────────────────┘   │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐   │
│  │ TaskMonitor  │  │  TaskGraph   │  │  ModelRouter         │   │
│  │  (progress)  │  │  (DAG model) │  │  (intelligent)       │   │
│  └──────────────┘  └──────────────┘  └──────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

### Core Components

| Component | Location | Description |
|-----------|----------|-------------|
| **cowork_types/** | `dispatcher/cowork_types/` | Task, TaskGraph, TaskDependency definitions |
| **planner/** | `dispatcher/planner/` | LLM-based task decomposition (llm.rs, prompt.rs) |
| **scheduler/** | `dispatcher/scheduler/` | DAG scheduling with topological sort |
| **executor/** | `dispatcher/executor/` | Task execution (file_ops.rs, code_exec.rs, permission.rs, noop.rs) |
| **monitor/** | `dispatcher/monitor/` | Real-time progress tracking and events |
| **model_router/** | `dispatcher/model_router/` | Intelligent model selection (5 sub-modules) |
| **engine.rs** | `dispatcher/engine.rs` | CoworkEngine unified API |

### Task Types

| Type | Description |
|------|-------------|
| `FileOperation` | Read, write, copy, delete files |
| `CodeExecution` | Run scripts or code |
| `DocumentGeneration` | Create documents, reports |
| `AppAutomation` | Control applications via AppleScript |
| `AiInference` | Call AI models for generation |

### DAG Scheduler

Executes tasks respecting dependencies with configurable parallelism:

1. Compute in-degree for all tasks
2. Queue tasks with zero in-degree
3. Execute up to `max_parallelism` tasks concurrently
4. When task completes, decrement dependents' in-degree
5. Queue newly ready tasks

### Tool Safety Levels

| Level | Description | Confirmation | Rollback |
|-------|-------------|--------------|----------|
| `ReadOnly` | No state changes (search, read) | No | N/A |
| `Reversible` | Can be undone (copy file, create) | If low confidence | Yes |
| `IrreversibleLowRisk` | Minor permanent changes | Yes | No |
| `IrreversibleHighRisk` | Major permanent changes (delete) | Always | No |

### Model Router

Intelligent model selection based on task type and capabilities:

| Sub-module | Location | Description |
|------------|----------|-------------|
| **core/** | `model_router/core/` | Core routing logic, profiles, rules, scoring |
| **health/** | `model_router/health/` | Health monitoring, status, metrics |
| **resilience/** | `model_router/resilience/` | Fault tolerance, retry, failover, budget |
| **intelligent/** | `model_router/intelligent/` | Smart routing P2 (prompt analysis, semantic cache) |
| **advanced/** | `model_router/advanced/` | Advanced features P3 (A/B testing, ensemble) |

**Cost Strategies**: `Cheapest`, `Balanced`, `BestQuality`

**Routing Priority**:
1. Explicit task type mapping
2. Required capability matching
3. Cost strategy application
4. Default model fallback
5. Fallback provider (`[general].default_provider`)

---

## Configuration

See [CONFIGURATION.md](./CONFIGURATION.md#dispatcher) for full configuration options.

```toml
[dispatcher]
enabled = true
confirmation_enabled = true
confirmation_threshold = 0.7
confirmation_timeout_ms = 30000

[cowork]
enabled = true
require_confirmation = true
max_parallelism = 4
dry_run = false
max_tasks_per_graph = 20
task_timeout_seconds = 300

[cowork.file_ops]
enabled = true
allowed_paths = ["~/Downloads/**", "~/Documents/**"]
max_file_size = "100MB"
require_confirmation_for_write = true
require_confirmation_for_delete = true

[cowork.code_exec]
enabled = false  # Disabled by default for security
default_runtime = "shell"
timeout_seconds = 60
sandbox_enabled = true
allowed_runtimes = ["shell", "python"]

[cowork.model_routing]
cost_strategy = "balanced"
default_model = "claude-sonnet"
enable_pipelines = true
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

## Tool Call Repair

When LLMs call tools with incorrect names, the system automatically attempts to repair the call:

### Repair Strategies

| Strategy | Example | Priority |
|----------|---------|----------|
| **Exact Match** | `web_search` → `web_search` | 1 |
| **Case-Insensitive** | `WebSearch` → `web_search` | 2 |
| **Snake Case Conversion** | `webSearch` → `web_search` | 3 |
| **Invalid Fallback** | `unknown` → `invalid` tool | 4 |

### Repair Flow

```
Tool Call: "WebSearch"
     │
     ├─► Exact match? ─── No
     │
     ├─► Case-insensitive? ─── Found: "web_search"
     │        │
     │        └─► Execute with repair info logged
     │
     └─► (If still not found)
              │
              └─► Route to InvalidTool
                       │
                       └─► Returns: "Tool 'WebSearch' not found.
                                    Available tools: search, web_fetch, ..."
```

### InvalidTool Response

When no match is found, the `invalid` tool provides helpful feedback:

```json
{
  "success": false,
  "message": "Tool 'unknown_tool' not found. Error: No matching tool in registry",
  "suggestion": "Available tools: search, web_fetch, youtube, file_ops, ... (and 10 more)"
}
```

This allows the LLM to self-correct on the next iteration.

---

## Code Locations

| Component | Location |
|-----------|----------|
| **Dispatcher module** | `core/src/dispatcher/` |
| Tool Registry | `dispatcher/registry.rs` |
| Dispatcher Engine | `dispatcher/engine.rs` |
| Confirmation | `dispatcher/confirmation.rs`, `dispatcher/async_confirmation.rs` |
| Integration | `dispatcher/integration.rs` |
| **Task Planning** | `dispatcher/planner/` (llm.rs, prompt.rs) |
| **Task Execution** | `dispatcher/executor/` (file_ops.rs, code_exec.rs, permission.rs) |
| **DAG Scheduling** | `dispatcher/scheduler/` |
| **Progress Monitoring** | `dispatcher/monitor/` |
| **Model Router** | `dispatcher/model_router/` (core/, health/, resilience/, intelligent/, advanced/) |
| **Cowork Types** | `dispatcher/cowork_types/` |
| **Thinker (LLM Decision)** | `thinker/` (prompt_builder.rs, decision_parser.rs, model_router.rs) |
| **Intent Detection** | `intent/detection/` (classifier.rs, ai_detector.rs) |
| **Intent Routing** | `intent/decision/router.rs` |
| Rollback Support | `intent/support/rollback.rs` |
| **Agent Loop** | `agent_loop/` (decision.rs, state.rs, guards.rs, callback.rs, config.rs) |
| **Executor** | `executor/` (single_step.rs, builtin_registry.rs) |
| **Rig Tools** | `rig_tools/` (search.rs, web_fetch.rs, file_ops.rs, youtube.rs, invalid.rs, skill_reader.rs) |
| **Tool Output** | `tool_output/` (truncation.rs, cleanup.rs) |
| **Tool Server** | `tools/server.rs` (call_with_repair, try_repair_tool_name) |
| Swift event handler | `platforms/macos/Aleph/Sources/EventHandler.swift` |
| Swift notifications | `platforms/macos/Aleph/Sources/Notifications.swift` |
| Command completion | `platforms/macos/Aleph/Sources/Utils/CommandCompletionManager.swift` |

---

**Last Updated**: 2026-01-24
