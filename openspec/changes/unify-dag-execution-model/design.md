# Design: Unify DAG Execution Model

## Context

### Current Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    CURRENT STATE                             │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  Path A: Agent Loop (Single Tool)                           │
│  ─────────────────────────────────                          │
│  Thinker → Decision::UseTool → Action::ToolCall → Result    │
│                                                              │
│  Path B: DAG Scheduler (Multi Task) - FFI Only              │
│  ─────────────────────────────────────────────              │
│  FFI → LlmTaskPlanner → TaskGraph → DagScheduler → Result   │
│                                                              │
│  ❌ Path A cannot trigger Path B                            │
│  ❌ LLM cannot express "run these tools in parallel"        │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### Target Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    TARGET STATE                              │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  Unified Agent Loop                                          │
│  ──────────────────                                         │
│                                                              │
│  Thinker → Decision → Action → ActionExecutor → Result      │
│                                                              │
│  Decision types:                                             │
│  ├─ UseTool       → ToolCall        → Single execution      │
│  ├─ UseTools      → ParallelCalls   → tokio::join_all       │
│  ├─ UseTools      → SequentialCalls → Chained execution     │
│  └─ ExecuteGraph  → GraphExecution  → DagScheduler          │
│                                                              │
│  Additional trigger:                                         │
│  └─ batch_execute tool → Internal UseTools decision         │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

## Goals / Non-Goals

### Goals
- Enable LLM to express multi-tool execution intent
- Integrate DagScheduler into Agent Loop
- Provide batch_execute tool for backward compatibility with opencode patterns
- Maintain full backward compatibility with existing single-tool flow
- Clean up redundant FFI DAG execution path

### Non-Goals
- Change DagScheduler's internal implementation
- Modify task planning logic (LlmTaskPlanner)
- Add new scheduling algorithms
- Support distributed execution

## Decisions

### Decision 1: Extend Decision enum (not replace)

**Choice**: Add new variants to existing Decision enum

**Rationale**:
- Maintains backward compatibility
- Single source of truth for LLM decisions
- Avoids parallel type hierarchies

**Alternatives considered**:
- Create separate `MultiDecision` type - rejected, adds complexity
- Use trait objects - rejected, loses type safety

### Decision 2: Two-tier multi-tool support

**Choice**: Support both native `UseTools` decision AND `batch_execute` tool

**Rationale**:
- Native `UseTools` is more efficient (no extra tool call overhead)
- `batch_execute` tool provides compatibility with opencode patterns
- LLM can choose based on complexity of task

**Implementation**:
```rust
// Native multi-tool decision (preferred)
Decision::UseTools {
    tools: vec![
        ToolCall { name: "search".into(), args: json!({...}) },
        ToolCall { name: "file_read".into(), args: json!({...}) },
    ],
    parallel: true,
}

// Tool-based batch execution (compatibility)
Decision::UseTool {
    tool_name: "batch_execute".into(),
    arguments: json!({
        "tools": [
            {"name": "search", "arguments": {...}},
            {"name": "file_read", "arguments": {...}},
        ],
        "parallel": true
    }),
}
```

### Decision 3: Graph execution through Agent Loop

**Choice**: Add `ExecuteGraph` decision type that delegates to DagScheduler

**Rationale**:
- Unifies all execution paths through Agent Loop
- Enables proper guard checking and state management
- Allows callback integration for UI updates

**Flow**:
```
LLM returns ExecuteGraph decision
    ↓
AgentLoop converts to GraphExecution action
    ↓
ActionExecutor calls DagScheduler::execute_graph()
    ↓
Results aggregated into GraphResult
    ↓
Fed back to LLM for next decision
```

### Decision 4: Parallel execution strategy

**Choice**: Use `tokio::join_all` for ParallelToolCalls

**Rationale**:
- Simple, well-understood concurrency model
- Proper error handling (all results, including failures)
- No need for complex task management

**Implementation**:
```rust
Action::ParallelToolCalls { calls } => {
    let futures: Vec<_> = calls.iter()
        .map(|call| self.execute_single_tool(call))
        .collect();

    let results = futures::future::join_all(futures).await;

    ActionResult::MultiToolResults {
        results: results.into_iter().map(|r| r.into()).collect(),
        total_duration_ms: start.elapsed().as_millis() as u64,
    }
}
```

### Decision 5: Limit batch size

**Choice**: Maximum 25 tools per batch (matching opencode)

**Rationale**:
- Prevents resource exhaustion
- Matches proven opencode limit
- Easy to adjust if needed

## Risks / Trade-offs

### Risk 1: LLM prompt complexity
- **Risk**: LLM may struggle with new response format
- **Mitigation**: Provide clear examples in system prompt, fallback parsing

### Risk 2: Error propagation
- **Risk**: Partial failures in parallel execution
- **Mitigation**: Return all results (success + failure), let LLM handle

### Risk 3: Resource contention
- **Risk**: 25 parallel API calls may cause rate limiting
- **Mitigation**: Consider semaphore-based concurrency limiting

## Data Model Changes

### New Types in decision.rs

```rust
/// Single tool call specification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub tool_name: String,
    pub arguments: Value,
}

/// Extended Decision enum
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Decision {
    // Existing variants (unchanged)
    UseTool { tool_name: String, arguments: Value },
    AskUser { question: String, options: Option<Vec<String>> },
    Complete { summary: String },
    Fail { reason: String },

    // NEW: Multi-tool execution
    UseTools {
        tools: Vec<ToolCall>,
        #[serde(default)]
        parallel: bool,  // true = parallel, false = sequential
    },

    // NEW: Graph execution
    ExecuteGraph {
        graph: TaskGraphSpec,
    },
}

/// Lightweight task graph specification for LLM
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskGraphSpec {
    pub tasks: Vec<TaskSpec>,
    pub dependencies: Vec<(String, String)>,  // (from, to)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskSpec {
    pub id: String,
    pub tool_name: String,
    pub arguments: Value,
}
```

### New Types in ActionResult

```rust
/// Individual tool result
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolResult {
    pub tool_name: String,
    pub success: bool,
    pub output: Value,
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// Extended ActionResult enum
pub enum ActionResult {
    // Existing variants (unchanged)
    ToolSuccess { output: Value, duration_ms: u64 },
    ToolError { error: String, retryable: bool },
    UserResponse { response: String },
    Completed,
    Failed,

    // NEW: Multi-tool results
    MultiToolResults {
        results: Vec<ToolResult>,
        total_duration_ms: u64,
    },

    // NEW: Graph execution result
    GraphResult {
        completed_tasks: Vec<String>,
        failed_tasks: Vec<String>,
        outputs: std::collections::HashMap<String, Value>,
        total_duration_ms: u64,
    },
}
```

## Migration Plan

### Phase 1: Type Extensions (Non-breaking)
1. Add new variants to Decision, Action, ActionResult
2. Implement serialization/deserialization
3. Add unit tests

### Phase 2: Executor Integration
1. Extend ActionExecutor to handle new action types
2. Implement parallel execution with join_all
3. Integrate DagScheduler for graph execution
4. Add integration tests

### Phase 3: batch_execute Tool
1. Implement batch_execute as AetherTool
2. Wire into tool registry
3. Test with LLM

### Phase 4: LLM Integration
1. Update prompt builder for new response format
2. Extend decision parser for multi-tool responses
3. Add examples to system prompt
4. End-to-end testing

### Phase 5: Cleanup
1. Deprecate ffi/dag_executor.rs::run_dag_execution()
2. Remove after transition period
3. Update documentation

## Open Questions

1. Should we support nested graphs (graph task can trigger another graph)?
   - Recommendation: No, keep it simple for v1

2. Should batch_execute support sequential mode?
   - Recommendation: Yes, via `parallel: false` parameter

3. How to handle timeout for parallel execution?
   - Recommendation: Global timeout, cancel all on expiry

## Key Files

| File | Changes |
|------|---------|
| `core/src/agent_loop/decision.rs` | Add new Decision/Action/ActionResult variants |
| `core/src/agent_loop/mod.rs` | Handle new action types in run() |
| `core/src/thinker/decision_parser.rs` | Parse multi-tool responses |
| `core/src/thinker/prompt_builder.rs` | Add examples for new formats |
| `core/src/rig_tools/batch_execute.rs` | NEW: batch_execute tool |
| `core/src/ffi/dag_executor.rs` | DEPRECATE: run_dag_execution |
