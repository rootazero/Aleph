# Tool Calling 2.0: Parallel Execution & Strict Mode

**Date**: 2026-03-11
**Status**: Approved
**Scope**: Parallel tool calling + Strict Mode (Option C). Tool RAG deferred to future iteration.

---

## Background

Aleph's tool calling pipeline currently processes only the first tool call from each LLM response (`tool_calls[0]`), even though both OpenAI and Anthropic protocols support multiple parallel calls per turn. This creates a "pseudo-2.0" bottleneck: iteration count doubles for tasks that could run tools concurrently, wasting tokens and wall-clock time.

Additionally, the lack of Strict Mode support means `DecisionParser` must maintain complex fallback/repair logic for malformed JSON arguments.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Scope | Parallel + Strict together | Strict Mode is the "fuse" for parallel — ensures all N tool args parse cleanly |
| Data model | Unified `UseTools(Vec<..>)` (Option 2) | Single code path; single call = N=1 special case |
| Error semantics | Independent result reporting (Option B) | Maximize information for LLM; no fake rollback |
| Tool RAG | Deferred | Different architectural layer (tool management vs execution protocol) |

---

## Section 1: Data Model Layer

### New Core Types

```rust
/// A single tool call record from LLM response, with provider-assigned ID.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub call_id: String,
    pub tool_name: String,
    pub arguments: Value,
}

/// A single tool call request ready for execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub call_id: String,
    pub tool_name: String,
    pub arguments: Value,
}

/// Result of a single tool execution, bound to its originating call_id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub call_id: String,
    pub tool_name: String,
    pub result: SingleToolResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SingleToolResult {
    Success { output: Value, duration_ms: u64 },
    Error { error: String, retryable: bool },
}
```

### Decision Enum

```rust
pub enum Decision {
    UseTools(Vec<ToolCallRecord>),    // replaces UseTool
    AskUser { question: String, options: Option<Vec<String>> },
    AskUserMultigroup { question: String, groups: Vec<QuestionGroup> },
    AskUserRich { question: String, kind: QuestionKind, question_id: Option<String> },
    Complete { summary: String },
    Fail { reason: String },
    Silent,
    HeartbeatOk,
}
```

`LlmAction` mirrors this change. `From<LlmAction> for Decision` updated accordingly.

### Action Enum

```rust
pub enum Action {
    ToolCalls(Vec<ToolCallRequest>),  // replaces ToolCall
    UserInteraction { ... },
    UserInteractionMultigroup { ... },
    UserInteractionRich { ... },
    Completion { summary: String },
    Failure { reason: String },
}
```

### ActionResult Enum

```rust
pub enum ActionResult {
    ToolResults(Vec<ToolCallResult>), // replaces ToolSuccess/ToolError
    UserResponse { response: String },
    UserResponseRich { response: UserAnswer },
    Completed,
    Failed,
}
```

### Thinking Struct

```rust
pub struct Thinking {
    pub reasoning: Option<String>,
    pub decision: Decision,
    pub structured: Option<StructuredThinking>,
    pub tokens_used: Option<usize>,
    // tool_call_id removed — IDs embedded in ToolCallRecord
}
```

### Transition Adapter (deprecated)

```rust
impl Decision {
    #[deprecated(note = "Migrate to handle UseTools batch directly")]
    pub fn as_single_tool(&self) -> Option<(&str, &Value)> {
        match self {
            Decision::UseTools(calls) if !calls.is_empty() =>
                Some((&calls[0].tool_name, &calls[0].arguments)),
            _ => None,
        }
    }
}
```

### Design Notes

- `ToolCallRecord` (Decision layer) vs `ToolCallRequest` (Action layer): same fields, separate types for future divergence (e.g., execution strategy fields on Request).
- `call_id` flows end-to-end: `NativeToolCall.id` -> `ToolCallRecord.call_id` -> `ToolCallRequest.call_id` -> `ToolCallResult.call_id`.
- `tool_name` kept in `ToolCallResult` for logging/debugging convenience.

---

## Section 2: Thinker Mapping Layer

### `build_thinking_from_native_response` Rewrite

Two-phase processing:

1. **Partition**: Split `tool_calls` into virtual (`__complete`, `__fail`, `__ask_user`) and real calls.
2. **Terminal defense**: If any virtual tool present, ignore all real tools and map to terminal decision.
3. **Batch mapping**: Otherwise, map all real calls to `Decision::UseTools(Vec<ToolCallRecord>)`.

### Terminal Action Priority

When multiple virtual tools conflict: `__fail` > `__ask_user` > `__complete`.

Rationale: `__fail` = model knows it can't proceed (most urgent); `__ask_user` = needs info; `__complete` = most optimistic, least trustworthy under conflict.

### JSON-in-text Path Adaptation

Non-native providers always produce `UseTools(vec![single_record])` with a synthetic `call_id` (`"synth_<uuid>"`). No parallel parsing attempted on this path.

### Function Decomposition

Split `map_native_tool_call_to_decision` into:
- `map_virtual_tool_to_decision(tc: &NativeToolCall) -> Decision` — terminal actions only
- `map_to_record(tc: &NativeToolCall) -> ToolCallRecord` — real tools only

---

## Section 3: Parallel Executor & Feedback Loop

### Execution Flow

```
Decision::UseTools(records)
  -> doom loop check (per individual call)
  -> batch confirmation (if any tool requires it)
  -> parallel execution via JoinSet
  -> collect & sort results (restore request order)
  -> emit per-tool and batch events
  -> build N tool-result messages (one per call_id)
  -> feed all messages back to LLM
```

### ActionExecutor Trait Extension

```rust
#[async_trait]
pub trait ActionExecutor: Send + Sync {
    async fn execute(&self, action: &Action, identity: &IdentityContext) -> ActionResult;

    async fn execute_parallel(&self, action: &Action, identity: &IdentityContext) -> ActionResult {
        // Default: sequential fallback
    }

    async fn execute_single_tool(&self, req: &ToolCallRequest, identity: &IdentityContext) -> ToolCallResult;
}
```

### Parallel Implementation

- **N=1 fast path**: Skip JoinSet overhead, direct execution.
- **N>1**: `tokio::task::JoinSet`, each tool spawned independently.
- **Result ordering**: Sort by original request position after collection (JoinSet returns completion order).
- **Panic defense**: `JoinError` converted to `SingleToolResult::Error`, batch continues.

### Feedback Messages

Each `ToolCallResult` becomes an independent `Message { role: Tool, tool_call_id: Some(call_id), content }`. `Message` struct gains `tool_call_id: Option<String>` field.

### Confirmation Semantics

Batch confirmation: if any tool in batch requires confirmation, present all such tools at once. User approves or denies entire batch — no partial approval.

### LoopStep Granularity

One LLM interaction = one `LoopStep`, regardless of how many tools executed. `steps.len()` remains 1:1 with LLM turns.

### New Counter

`LoopState.total_tool_calls: usize` — cumulative tool invocation count across all steps.

---

## Section 4: Strict Mode Integration

### AlephTool Trait Extension

```rust
fn strict_schema(&self) -> bool { true }  // default: opt-in
```

Tools with dynamic/flexible schemas override to `false`.

### ToolDefinition Extension

```rust
pub struct ToolDefinition {
    // ... existing fields ...
    pub strict: bool,  // new
}
```

### Schema Strictification

New module `src/tools/schema_strictify.rs`:
- `strictify_schema(schema: &mut Value)` — recursive transform:
  - Set `additionalProperties: false` on all object types
  - Make all properties `required`
  - Recurse into nested schemas (`properties`, `items`, `allOf`, etc.)
- Applied once in `collect_native_tool_defs()` for tools with `strict: true`.

### Provider Adaptation

| Provider | Strict support |
|----------|---------------|
| OpenAI | `"strict": true` injected in function definition |
| Anthropic | No explicit flag; strictified schema still improves output quality |

### DecisionParser Simplification

Strict mode path skips fallback/repair logic — trusts schema enforcement. Non-strict and JSON-in-text paths retain full fallback capability.

### Virtual Tools

Always strict. Their schemas are fixed and simple.

---

## Section 5: Error Handling, Edge Cases & Observability

### Error Semantics (Independent Reporting)

Each tool result reported independently. Success and failure coexist in same batch. LLM decides whether to retry failed items.

### Post-Execution Validation

Assert `requests.len() == results.len()` and all `call_id` match. Debug: assert; Production: warn + continue.

### Doom Loop Detection

Granularity: per individual tool call against recent history. `[read_A, read_B]` + `[read_A, read_C]` = not a doom loop. Three consecutive `read_A` with same args = doom loop.

### Event Emission

```
ToolBatchStarted { call_ids, tool_names }
ToolCompleted { call_id, tool_name, success, duration_ms }  // per tool, as-completed
ToolBatchCompleted { total, succeeded, failed }
```

### Concurrency Safety

- Tools receive `&self` (read-only) + cloned identity. No shared mutable state at framework level.
- File system conflicts (e.g., concurrent write + read to same file) are LLM planning errors, not framework concerns.
- No artificial concurrency limit on JoinSet (LLM rarely returns >5 parallel calls).

### Observability

Wall-clock time = slowest tool in batch. Logged at INFO level. Per-tool timing at DEBUG.

### Token & Step Accounting

| Metric | Change |
|--------|--------|
| `tokens_used` | Unchanged (per LLM turn) |
| `step_count` | Unchanged (1 LLM turn = 1 step) |
| `total_tool_calls` | New: cumulative across all steps |

---

## Files Affected (Estimated)

| File | Change Type |
|------|-------------|
| `src/agent_loop/decision.rs` | Major: enum restructure |
| `src/agent_loop/state.rs` | Moderate: Thinking, LoopStep, LoopState |
| `src/agent_loop/agent_loop.rs` | Major: execution loop, feedback |
| `src/agent_loop/traits.rs` | Moderate: ActionExecutor extension |
| `src/thinker/mod.rs` | Major: parallel mapping, virtual tool defense |
| `src/thinker/decision_parser.rs` | Moderate: strict mode bypass, UseTools wrapping |
| `src/thinker/virtual_tools.rs` | Minor: strict flag on virtual defs |
| `src/tools/traits.rs` | Minor: `strict_schema()` method |
| `src/tools/schema_strictify.rs` | New file |
| `src/dispatcher/types/definition.rs` | Minor: `strict` field, serialization |
| `src/providers/adapter.rs` | Minor: no structural change |
| `src/providers/protocols/openai.rs` | Minor: strict flag in tool def |
| `src/agent_loop/message_builder.rs` | Moderate: batch message generation |
| All match sites on `Decision::UseTool` | Mechanical: rename to `UseTools` pattern |

## Implementation Strategy

Recommended approach: **Compiler-driven refactoring**. Change `Decision` enum first, let `cargo check` surface all match sites, fix them systematically. This is the safest path for a system-wide type change.
