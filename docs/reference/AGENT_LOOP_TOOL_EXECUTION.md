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

**Active consumers (verified via codebase grep 2026-04-19):**

A full grep for `ToolPipeline|PipelineOutcome|partition_tool_calls|execute_tool_batch|ToolExecutionContext|CascadePolicy|ToolProgress|ToolOutcome` across `src/` returns 8 files:

- `src/agent_loop/tool_pipeline.rs` (self)
- `src/agent_loop/tool_orchestrator.rs` (self)
- `src/agent_loop/tool_execution_context.rs` (self)
- `src/agent_loop/mod.rs` — re-exports `PipelineOutcome` and `ToolPipeline` publicly
- `src/agent_loop/loop_core.rs` — **real consumer**
- `src/session/streaming.rs` — **real consumer**
- `src/exec/sandbox/audit.rs` — **name collision only** (defines its own local `pub struct ToolExecutionContext` with fields `tool_name`, `tool_version`, `base_preset`, `applied_overrides`, etc.; no import of `agent_loop::tool_execution_context::ToolExecutionContext`)
- `src/builtin_tools/mod.rs` — **name collision only** (defines its own `ToolProgressCallback` trait, matched via substring on `ToolProgress`; no import of `agent_loop::tool_execution_context::ToolProgress`)

Cross-checked with a strict import grep (`use .*agent_loop::(tool_pipeline|tool_orchestrator|tool_execution_context)`): only `session/streaming.rs` uses the explicit path. `loop_core.rs` uses `super::tool_pipeline::{PipelineOutcome, ToolPipeline}`. No other module in the crate imports these types.

**Real external consumer map:**

- `src/agent_loop/loop_core.rs` — stores `Arc<ToolPipeline>` as a field on the loop struct (line 681), constructs it in `new()` (line 759), rebuilds it on hook changes (line 940), and stores `Vec<PipelineOutcome>` on an internal `executor_handle: JoinHandle<Vec<PipelineOutcome>>` (line 408). Real runtime dependency — the `ToolPipeline` is threaded through the full loop execution path.
- `src/session/streaming.rs` — imports `CascadePolicy`, `ToolProgress`, `ToolOutcome`, `PipelineOutcome`, `ToolPipeline` (lines 24-26). Implements a parallel streaming executor built on these types: `Arc<ToolPipeline>` on the executor struct (lines 75, 156), emits `Vec<PipelineOutcome>` from `run()` (line 162), dispatches on `CascadePolicy::classify` at runtime to decide whether to abort siblings (lines 232-233), constructs `PipelineOutcome`/`ToolOutcome` for synthetic abort results (lines 248, 322). Deep runtime dependency — not just type references.
- `src/agent_loop/mod.rs` — re-exports `PipelineOutcome` and `ToolPipeline` as part of the crate's public `agent_loop` API surface (line 58).

**Implication:** Phase 5/6 **cannot delete these files outright**. Consumers must be migrated first:

1. Phase 5 migrates `loop_core.rs` off `ToolPipeline` onto the new Harness / `ToolService` middleware path.
2. Phase 5/6 migrates `session/streaming.rs` — the parallel streaming executor — to the replacement abstractions (likely a ToolService-aware streaming wrapper that preserves `CascadePolicy` semantics and progress channels).
3. Only once both consumers no longer reference the types can the three residue files be deleted and their `pub use` in `agent_loop/mod.rs` removed.

**Prior draft error:** An earlier version of this section claimed "zero call sites exist outside the files themselves." That was wrong — it was based on a grep whose `-v` filter inadvertently excluded the real consumer files. A code-quality reviewer caught the factual error; the corrected consumer map above supersedes it.

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

**External consumer:** `src/session/streaming.rs` imports `CascadePolicy` and `ToolProgress`
directly and dispatches on `CascadePolicy::classify` at runtime (line 232). The file cannot be
deleted in Phase 6 until `streaming.rs` is migrated off these types.

**Decision:** Leave in `src/agent_loop/`. No code moved. No TODO(Phase 5) inside the file
(the types themselves are small — the work is on the consumer side, not here). Deletion deferred
until `session/streaming.rs` migrates to replacement abstractions.

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

**External consumer:** `src/session/streaming.rs` imports `ToolOutcome` and constructs it for
synthetic abort results (lines 248, 322). `src/agent_loop/tool_pipeline.rs` also imports
`ToolOutcome` internally. Deletion in Phase 6 requires migrating `streaming.rs` off `ToolOutcome`.

**Decision:** Leave in `src/agent_loop/`. No code moved. Added `// TODO(Phase 5)` marker at
top of file. `execute_tool_batch` is the primary migration target for Phase 5; `streaming.rs`
consumer must be migrated before the file can be deleted.

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

Also handles: output compression (`compress_tool_output`), ingress hygiene
(`tool_output::hygiene::clean_result_value` — content-type reduction + error
distillation, applied to the tool's *structured value* before it is flattened;
see FEATURE_LOCATOR §3.14), token-budget truncation (head+tail), disk offloading
via `ToolResultStore`, file-read tracking via `FileContentTracker`, and
`RwLock<String>`-guarded session_id updates.

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

**External consumers:**
- `src/agent_loop/loop_core.rs` — stores `Arc<ToolPipeline>` as a field on the loop struct
  (line 681), constructs it in `new()` (line 759), rebuilds it when hooks change (line 940),
  and stores `Vec<PipelineOutcome>` on a `JoinHandle<Vec<PipelineOutcome>>` (line 408). This is
  the primary runtime dependency.
- `src/session/streaming.rs` — stores `Arc<ToolPipeline>` on a parallel streaming executor
  (lines 75, 156) and returns `Vec<PipelineOutcome>` from `run()` (line 162).
- `src/agent_loop/mod.rs` — re-exports `PipelineOutcome` and `ToolPipeline` as public API.

**Decision:** Leave in `src/agent_loop/`. No code moved (pragmatic rule applied: deeply
entangled with loop internals). Added `// TODO(Phase 5)` marker at top of file. In Phase 5/6,
the goal is to retire `LoopToolRegistry` entirely and route all tool calls through `ToolService`,
at which point `ToolPipeline` is replaced by ToolService middleware — **but only after both
`loop_core.rs` and `streaming.rs` are migrated off the `ToolPipeline` type**.

---

### Summary Table

| File | Lines | ToolService-layer moved | External consumers | Disposition |
|------|-------|------------------------|--------------------|-------------|
| `tool_execution_context.rs` | 148 | 0 lines | `session/streaming.rs` (imports `CascadePolicy`, `ToolProgress`) | Migrate consumer first, then delete |
| `tool_orchestrator.rs` | 586 | 0 lines | `session/streaming.rs` (imports `ToolOutcome`); `agent_loop/tool_pipeline.rs` (internal) | Phase 5 Orchestrator seed; TODO(Phase 5); migrate `streaming.rs` before delete |
| `tool_pipeline.rs` | 1363 | 0 lines | `loop_core.rs` (`Arc<ToolPipeline>` field, rebuilt on hook changes); `session/streaming.rs` (`Arc<ToolPipeline>` on parallel executor); re-exported via `agent_loop/mod.rs` | Loop-internal; TODO(Phase 5); migrate both consumers before delete |

**Total code moved to `src/tools/`:** 0 lines (pragmatic rule applied to all three files).

**Why nothing moved:** The ToolService decorator chain (`src/tools/`) already covers its domain
correctly. The three residue files operate on a different registry (`LoopToolRegistry`) and are
part of a parallel execution path that will be eliminated — not extended — in Phase 5/6 when
`LoopToolRegistry` is retired and all tools flow through `ToolService`.

**Phase 5/6 deletion order (corrected):** These files cannot be deleted "outright" in Phase 6.
The two real external consumers must be migrated first:

1. Migrate `loop_core.rs` off `Arc<ToolPipeline>` to the new Harness / `ToolService` middleware.
2. Migrate `session/streaming.rs` — port its parallel streaming executor to ToolService-aware
   abstractions that preserve `CascadePolicy` semantics and progress-channel streaming.
3. Remove the three files and their `pub use` / `pub mod` declarations in `agent_loop/mod.rs`.

**`loop_core.rs` line count:** 4558 lines before and after Task 6 (no change — Task 4 was
skipped, so the planned 1700-line reduction did not occur; Phase 6 deletion will handle this,
but only after the migrations above).
