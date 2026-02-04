# Change: Unify DAG Execution Model

## Why

Aleph currently has **two disconnected execution systems**:

1. **Agent Loop** (`agent_loop/mod.rs`) - Single-tool sequential execution via `Decision::UseTool`
2. **DAG Scheduler** (`dispatcher/scheduler/dag.rs`) - Full DAG parallel execution, but only accessible via FFI

This architecture gap prevents the LLM from leveraging parallel execution capabilities. When the LLM needs to execute multiple independent tools (e.g., search + file_read + web_fetch), it must:
- Execute them sequentially across multiple loop iterations
- Or rely on external FFI calls to access DAG scheduling

**Reference**: opencode provides `BatchTool` that allows LLM to batch execute up to 25 tools in parallel, solving this problem elegantly.

## What Changes

### Core Type Extensions

1. **Decision enum** - Add multi-tool and graph execution variants:
   - `UseTools { tools: Vec<ToolCall>, parallel: bool }` - Batch tool execution
   - `ExecuteGraph { graph: TaskGraphSpec }` - Full DAG execution

2. **Action enum** - Add corresponding action types:
   - `ParallelToolCalls { calls: Vec<ToolCallSpec> }`
   - `SequentialToolCalls { calls: Vec<ToolCallSpec> }`
   - `GraphExecution { graph: TaskGraph }`

3. **ActionResult enum** - Add multi-result types:
   - `MultiToolResults { results: Vec<ToolResult>, total_duration_ms: u64 }`
   - `GraphResult { execution_result: ExecutionResult }`

### New Tool: batch_execute

Add `batch_execute` as a native AlephTool that allows LLM to:
- Execute multiple tool calls in parallel (up to 25)
- Aggregate results and return them in a structured format

### Execution Integration

1. **Unified Executor** - Extend `ActionExecutor` to handle new action types:
   - `ParallelToolCalls` → `tokio::join_all()` parallel execution
   - `SequentialToolCalls` → Sequential with context passing
   - `GraphExecution` → Delegate to `DagScheduler::execute_graph()`

2. **Deprecate FFI DAG path** - Remove `ffi/dag_executor.rs::run_dag_execution()` in favor of Agent Loop integration

### LLM Response Format Extension

Extend `LlmResponse` to support multi-tool decisions:
```json
{
  "reasoning": "...",
  "action": {
    "type": "tools",
    "tools": [
      {"tool_name": "search", "arguments": {...}},
      {"tool_name": "file_read", "arguments": {...}}
    ],
    "parallel": true
  }
}
```

## Impact

- **Affected specs**: New `agent-loop` spec (this change creates it)
- **Affected code**:
  - `core/src/agent_loop/decision.rs` - Type extensions
  - `core/src/agent_loop/mod.rs` - Execution logic
  - `core/src/thinker/decision_parser.rs` - Response parsing
  - `core/src/thinker/prompt_builder.rs` - Prompt format
  - `core/src/rig_tools/` - New batch_execute tool
  - `core/src/ffi/dag_executor.rs` - Deprecation/removal
- **Breaking changes**: None (additive changes, existing behavior preserved)

## Benefits

1. **Performance** - Independent tools execute in parallel
2. **Flexibility** - LLM can choose optimal execution strategy
3. **Simplicity** - Single unified execution path
4. **Consistency** - Match opencode's proven BatchTool pattern
