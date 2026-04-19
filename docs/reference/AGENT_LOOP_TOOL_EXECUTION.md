# Agent Loop Tool Execution

> Tool execution context, progress tracking, and pipeline architecture.

## Overview

Tools are executed through a 7-stage pipeline with comprehensive progress tracking and error recovery.

## Tool Pipeline Stages

```
┌─────────┐   ┌─────────┐   ┌─────────┐   ┌─────────┐   ┌─────────┐   ┌─────────┐   ┌─────────┐
│ Approve │ → │ Invoke  │ → │ Collect │ → │ Merge   │ → │ Compact │ → │ Budget  │ → │ Return  │
│         │   │         │   │ Results │   │ Results │   │ Results │   │ Check   │   │         │
└─────────┘   └─────────┘   └─────────┘   └─────────┘   └─────────┘   └─────────┘   └─────────┘
```

## ToolExecutionContext

```rust
pub struct ToolExecutionContext {
    pub tool_name: String,
    pub max_tool_result_tokens: usize,
    pub truncate_to_tokens: usize,
    pub truncation_policy: TruncationPolicy,
    pub cascade_policy: CascadePolicy,
}
```

## CascadePolicy

```rust
pub enum CascadePolicy {
    /// Abort all sibling tools when one fails
    AbortSiblings,
    /// Run all tools regardless of failures
    Isolated,
}
```

## ToolProgress Events

```rust
pub enum ToolProgress {
    Started { tool_name: String, invocation_id: String },
    Approved { tool_name: String },
    Rejected { tool_name: String, reason: String },
    Completed { tool_name: String, result_tokens: usize },
    Failed { tool_name: String, error: String },
}
```

## ToolCall Structure

```rust
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
    pub invocation_id: String,
}
```

## ToolResult Structure

```rust
pub struct ToolResult {
    pub invocation_id: String,
    pub content: ToolResultContent,
    pub error: Option<String>,
}

pub enum ToolResultContent {
    Text(String),
    Image { data: String, media_type: String },
    Audio { data: String, media_type: String },
}
```

## Approval Flow

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│  ToolCall    │ ──▶ │  Approval    │ ──▶ │   Executor   │
│  Received    │     │   Gate       │     │              │
└──────────────┘     └──────────────┘     └──────────────┘
                           │
                           ▼
                     ┌──────────────┐
                     │   Rejected   │ ──▶ Return error to LLM
                     └──────────────┘
```

## Result Merging

When multiple tools run in parallel, results are merged based on `CascadePolicy`:

- **AbortSiblings**: First failure aborts remaining tools
- **Isolated**: All tools complete, results merged regardless of failures

## Result Compact

After merging, results pass through compact stage which applies `TruncationPolicy`:

```rust
pub enum TruncationPolicy {
    HeadAndTail { keep_head_tokens: usize, keep_tail_tokens: usize },
    HeadOnly { keep_tokens: usize },
    TailOnly { keep_tokens: usize },
}
```

## Related Documents

- [AGENT_LOOP_CONTEXT_BUDGET.md](./AGENT_LOOP_CONTEXT_BUDGET.md) - Context budget tiers
- [AGENT_LOOP_RECOVERY.md](./AGENT_LOOP_RECOVERY.md) - Recovery from truncation errors

---

## Phase 4 Residue Audit (2026-04-19)

### Context

Phase 2 moved the canonical tool dispatch path into `src/tools/` (ToolService decorator chain:
`ExecAuditLayer → PermissionLayer → ContextRuleLayer → TimeoutLayer → CoreDispatch → ToolRegistry`).
Three files remained in `src/agent_loop/` with unabsorbed logic. This section records the
classification of each file as part of Phase 4a Task 6.

**Key finding:** All three files are declared `pub mod` in `src/agent_loop/mod.rs` and compile
cleanly, but **zero call sites exist outside the files themselves** (confirmed by full-codebase
grep). They are live code with no active consumers — orphaned by Phase 2.

---

### File 1: `src/agent_loop/tool_execution_context.rs` (148 lines)

**What it does today:** Defines three types for streaming/cancellation within a parallel tool
batch: `CascadePolicy` (AbortSiblings vs Isolated), `ToolProgress` (Status/PartialOutput events),
and `ToolExecutionContext` (wraps a `CancellationToken` child + `mpsc::Sender<ToolProgress>`).
`CascadePolicy::classify` hard-codes which tool names are side-effecting.

**ToolService-layer responsibilities:** None. ToolService is single-tool dispatch; cancellation
token trees and streaming progress channels are a loop-execution concern, not a service concern.

**Orchestrator-layer responsibilities:** `CascadePolicy` and `ToolExecutionContext` are the
building blocks for parallel batch execution — they belong with whatever coordinates parallel tool
runs. This makes them Phase 5 (Orchestrator) material once that layer exists.

**Loop-internal responsibilities:** All of it. Until Phase 5, these primitives belong here.

**Decision:** Leave in `src/agent_loop/`. No code moved. Marked for Phase 6 cleanup
(no TODO(Phase 5) needed — the types are small enough to absorb or re-define at Phase 5 time).

---

### File 2: `src/agent_loop/tool_orchestrator.rs` (586 lines)

**What it does today:** Partitions a `&[NativeToolCall]` slice into concurrent vs exclusive
batches (via `LoopToolRegistry::is_concurrent_safe`), then runs concurrent batches with
`futures::future::join_all` and exclusive tools sequentially. Emits `LoopTraceEvent` callbacks
before/after each tool. Delegates single-tool execution to `ToolPipeline::execute`. Returns
results sorted to match original input order.

**ToolService-layer responsibilities:** None. ToolService processes one call at a time. Batch
partitioning and `join_all` parallelism are loop-scheduling concerns, not dispatch concerns. There
is no duplication with `src/tools/`.

**Orchestrator-layer responsibilities:** This file IS the embryonic Orchestrator for Phase 5.
`execute_tool_batch` is the exact function that Phase 5's `Orchestrator` struct will generalise
(adding sub-agent delegation, work queues, etc.). The `partition_tool_calls` heuristic and
`Batch` enum are Phase 5 design material.

**Loop-internal responsibilities:** The `LoopCallback` / `LoopTraceEvent` wiring ties this to the
current loop, but that is thin glue — the core logic is Orchestrator-layer.

**Decision:** Leave in `src/agent_loop/`. No code moved. Added `// TODO(Phase 5)` marker at
top of file. `execute_tool_batch` is the primary migration target for Phase 5.

---

### File 3: `src/agent_loop/tool_pipeline.rs` (1363 lines)

**What it does today:** Implements a 7-stage per-tool execution pipeline:
1. Build `HookContext` from metadata
2. Input schema validation (fast-fail)
3. Pre-hooks (`HookExecutor::execute_interceptors`) — may block, deny, ask, allow, or modify args
4. Safety check (`SafetyGuard`) — blocked patterns and permission policy
5. Execute tool via `LoopToolRegistry::execute` with `tokio::select!` cancellation
6. Post-hooks (`AfterToolCall` observers) — inject context, modify output
7. Failure hooks (`AfterToolCallFailure` observers) — fire on error outcomes

Also handles: output compression (`compress_tool_output`), token-budget truncation
(head+tail), disk offloading via `ToolResultStore`, file-read tracking via `FileContentTracker`,
and `RwLock<String>`-guarded session_id updates.

**ToolService-layer responsibilities (candidate to move):** At first glance, stages 2–4 resemble
the ToolService decorator chain (`PermissionLayer`, `ContextRuleLayer`, `TimeoutLayer`). However,
`ToolPipeline` operates on `LoopToolRegistry` (the `LoopTool` trait — loop-internal) while
`ToolService` operates on `ToolRegistry` (the `ToolHandler` trait — Phase 2 infrastructure).
These are **different registries with different traits**; the pipeline is not a duplicate of
the ToolService chain, it is a parallel path for loop-local tools. The hook/safety logic here
uses `HookExecutor` + `SafetyGuard` directly, while ToolService uses `PermissionLayer` wrapping
the same guards. This dual-path will be unified in Phase 5/6 when `LoopToolRegistry` is retired.

**Orchestrator-layer responsibilities:** None directly. `ToolPipeline` processes a single tool
call; `tool_orchestrator.rs` is the orchestration layer above it. `ToolPipeline` is a component
the Phase 5 Orchestrator will consume.

**Loop-internal responsibilities:** The `RwLock<String>` session_id, `FileContentTracker`,
`ToolResultStore`, and the hard dependency on `LoopToolRegistry` make this deeply loop-internal.
Extracting slices would increase coupling, not reduce it.

**Decision:** Leave in `src/agent_loop/`. No code moved (pragmatic rule applied: deeply
entangled with loop internals). Added `// TODO(Phase 5)` marker at top of file. In Phase 5/6,
the goal is to retire `LoopToolRegistry` entirely and route all tool calls through `ToolService`,
at which point `ToolPipeline` is replaced by ToolService middleware.

---

### Summary Table

| File | Lines | ToolService-layer moved | Disposition |
|------|-------|------------------------|-------------|
| `tool_execution_context.rs` | 148 | 0 lines | Loop-internal; leave, delete in Phase 6 |
| `tool_orchestrator.rs` | 586 | 0 lines | Phase 5 Orchestrator seed; TODO(Phase 5) |
| `tool_pipeline.rs` | 1363 | 0 lines | Loop-internal; TODO(Phase 5) |

**Total code moved to `src/tools/`:** 0 lines (pragmatic rule applied to all three files).

**Why nothing moved:** The ToolService decorator chain (`src/tools/`) already covers its domain
correctly. The three residue files operate on a different registry (`LoopToolRegistry`) and are
part of a parallel execution path that will be eliminated — not extended — in Phase 5/6 when
`LoopToolRegistry` is retired and all tools flow through `ToolService`.

**`loop_core.rs` line count:** 4558 lines before and after Task 6 (no change — Task 4 was
skipped, so the planned 1700-line reduction did not occur; Phase 6 deletion will handle this).
