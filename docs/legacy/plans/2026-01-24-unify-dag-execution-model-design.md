# Unify DAG Execution Model - Design Document

**Date**: 2026-01-24
**Status**: Approved
**OpenSpec Change**: `unify-dag-execution-model`

## Problem Statement

Aleph currently has two disconnected execution systems:

1. **Agent Loop** - Single-tool sequential execution via `Decision::UseTool`
2. **DAG Scheduler** - Full parallel execution, but only via FFI

This prevents the LLM from leveraging parallel execution capabilities.

## Solution Overview

Extend the Agent Loop to support:
1. Native multi-tool decisions (`Decision::UseTools`)
2. Graph execution decisions (`Decision::ExecuteGraph`)
3. `batch_execute` tool for opencode compatibility

## Type Extensions

### Decision Enum

```rust
pub enum Decision {
    // Existing (unchanged)
    UseTool { tool_name: String, arguments: Value },
    AskUser { question: String, options: Option<Vec<String>> },
    Complete { summary: String },
    Fail { reason: String },

    // NEW
    UseTools {
        tools: Vec<ToolCall>,
        parallel: bool,  // true=parallel, false=sequential
    },
    ExecuteGraph {
        graph: TaskGraphSpec,
    },
}
```

### Action Enum

```rust
pub enum Action {
    // Existing (unchanged)
    ToolCall { tool_name: String, arguments: Value },
    UserInteraction { ... },
    Completion { ... },
    Failure { ... },

    // NEW
    ParallelToolCalls { calls: Vec<ToolCallSpec> },
    SequentialToolCalls { calls: Vec<ToolCallSpec> },
    GraphExecution { graph: TaskGraph },
}
```

### ActionResult Enum

```rust
pub enum ActionResult {
    // Existing (unchanged)
    ToolSuccess { output: Value, duration_ms: u64 },
    ToolError { error: String, retryable: bool },
    UserResponse { response: String },
    Completed,
    Failed,

    // NEW
    MultiToolResults {
        results: Vec<ToolResult>,
        total_duration_ms: u64,
    },
    GraphResult {
        completed_tasks: Vec<String>,
        failed_tasks: Vec<String>,
        outputs: HashMap<String, Value>,
        total_duration_ms: u64,
    },
}
```

## Execution Flows

### Parallel Execution (UseTools with parallel=true)

```
LLM returns Decision::UseTools { tools: [A, B, C], parallel: true }
    ↓
AgentLoop converts to Action::ParallelToolCalls
    ↓
ActionExecutor::execute()
    ├─ tokio::join_all([execute(A), execute(B), execute(C)])
    ↓
ActionResult::MultiToolResults { results: [...] }
```

### Sequential Execution (UseTools with parallel=false)

```
LLM returns Decision::UseTools { tools: [A, B, C], parallel: false }
    ↓
AgentLoop converts to Action::SequentialToolCalls
    ↓
ActionExecutor::execute()
    ├─ result_A = execute(A)
    ├─ result_B = execute(B, context: result_A)
    └─ result_C = execute(C, context: result_A + result_B)
    ↓
ActionResult::MultiToolResults { results: [...] }
```

### Graph Execution (ExecuteGraph)

```
LLM returns Decision::ExecuteGraph { graph }
    ↓
AgentLoop converts to Action::GraphExecution
    ↓
ActionExecutor::execute()
    ├─ Convert TaskGraphSpec → TaskGraph
    └─ DagScheduler::execute_graph()
    ↓
ActionResult::GraphResult { completed, failed, outputs }
```

## batch_execute Tool

```rust
pub struct BatchExecute;

impl AlephTool for BatchExecute {
    const NAME: &'static str = "batch_execute";
    const DESCRIPTION: &'static str =
        "Execute multiple tools in a single call for better performance.";

    type Args = BatchExecuteArgs;
    type Output = BatchExecuteResult;
}

#[derive(JsonSchema)]
pub struct BatchExecuteArgs {
    pub tools: Vec<BatchToolCall>,  // max 25
    #[serde(default = "true")]
    pub parallel: bool,
}
```

## LLM Response Formats

### Multi-tool Decision

```json
{
  "reasoning": "Need to fetch multiple files in parallel",
  "action": {
    "type": "tools",
    "tools": [
      {"tool_name": "file_read", "arguments": {"path": "/src/main.rs"}},
      {"tool_name": "file_read", "arguments": {"path": "/src/lib.rs"}}
    ],
    "parallel": true
  }
}
```

### Graph Decision

```json
{
  "reasoning": "Need to execute tasks with dependencies",
  "action": {
    "type": "execute_graph",
    "graph": {
      "tasks": [
        {"id": "search", "tool_name": "web_search", "arguments": {...}},
        {"id": "write", "tool_name": "file_write", "arguments": {...}}
      ],
      "dependencies": [["search", "write"]]
    }
  }
}
```

## Migration Plan

1. **Phase 1**: Type extensions (non-breaking)
2. **Phase 2**: Executor integration
3. **Phase 3**: batch_execute tool
4. **Phase 4**: LLM integration
5. **Phase 5**: Cleanup and deprecation

## Files to Modify

| File | Changes |
|------|---------|
| `core/src/agent_loop/decision.rs` | Add new variants |
| `core/src/agent_loop/mod.rs` | Handle new action types |
| `core/src/thinker/decision_parser.rs` | Parse multi-tool responses |
| `core/src/thinker/prompt_builder.rs` | Add examples |
| `core/src/rig_tools/batch_execute.rs` | NEW: batch_execute tool |
| `core/src/ffi/dag_executor.rs` | DEPRECATE |

## References

- OpenSpec change: `openspec/changes/unify-dag-execution-model/`
- opencode BatchTool: `/Users/zouguojun/Workspace/opencode/packages/opencode/src/tool/batch.ts`
