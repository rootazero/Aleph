# Memory Reembed Migration Design

**Date**: 2026-03-20
**Status**: Approved

## Problem

Aleph's memory system stores embeddings in LanceDB across two tables (facts, memories) with three co-existing vector columns (`vec_768`, `vec_1024`, `vec_1536`). When users switch embedding providers (e.g., OpenAI 1536-dim to bge-m3 768-dim), old data's vectors don't match the new provider, breaking search. Users need a way to re-embed all old data with the new provider.

## Design Decisions

- **Approach**: Unified `reembed_all()` function handling both facts and memories in sequence
- **Old vector cleanup**: Clear old dimension columns on successful re-embed (delete+insert naturally achieves this)
- **Concurrency**: Serial batch processing (one batch at a time, KISS for low-frequency operation)
- **Failure handling**: Skip failed items, report in result; idempotent re-trigger fills gaps
- **Scope**: RPC + Panel only; builtin tool deferred to later
- **Namespace**: Reembed processes ALL memories across all namespaces (owner, guest, shared) — the goal is dimension alignment, not data filtering
- **Cancellation**: `CancellationToken` (AtomicBool) checked between batches for user-initiated abort

## Architecture

### 1. Data Layer Extension

#### SessionStore trait addition

```rust
// src/memory/store/mod.rs
async fn get_all_memories(&self, workspace: Option<&str>) -> Result<Vec<MemoryEntry>, AlephError>;
```

- LanceDB implementation in `sessions.rs`: full table scan with optional workspace filter
- No `update_memory()` needed — use existing `delete_memory()` + `insert_memory()` combo

### 2. Core Reembed Module

#### Data structures

```rust
// src/memory/reembed.rs

pub struct ReembedProgress {
    pub phase: &'static str,    // "facts" or "memories"
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
}

pub struct ReembedResult {
    pub facts_updated: usize,
    pub facts_total: usize,
    pub memories_updated: usize,
    pub memories_total: usize,
    pub errors: Vec<String>,
}
```

#### Unified entry point

```rust
pub async fn reembed_all(
    database: &MemoryBackend,
    embedder: &Arc<dyn EmbeddingProvider>,
    target_dim: usize,
    batch_size: usize,
    progress_tx: Option<tokio::sync::watch::Sender<ReembedProgress>>,
    cancel: Arc<AtomicBool>,    // checked between batches
) -> Result<ReembedResult, AlephError>
```

#### Internal flow

1. **Phase "facts"**:
   - `get_all_facts(true, None)` — fetch all including invalid
   - Filter: `embedding.is_none() || embedding.len() != target_dim`
   - Serial batch embed using `fact.content` as text
   - Update via `update_fact()` (delete+insert) — Arrow serialization only writes matching dimension column, old columns become null
   - Report progress via `progress_tx` after each batch
   - Skip failures, collect into `errors`

2. **Phase "memories"**:
   - `get_all_memories(None)` — fetch all
   - Filter same as facts
   - Embed text: `format!("{}\n\n{}", entry.user_input, entry.ai_output)` (matches ingestion.rs)
   - Update via `delete_memory()` + `insert_memory()`
   - Same progress and error handling

#### Old column cleanup

No special handling needed. The delete+insert pattern means Arrow serialization writes only the matching dimension column (e.g., 768-dim → `vec_768`), leaving `vec_1024` and `vec_1536` as null.

#### Idempotency

Records where `embedding.len() == target_dim` are skipped on filter. Re-triggering only processes remaining failures.

#### Cancellation

`cancel: Arc<AtomicBool>` is checked between batches. If set, the function returns early with a partial `ReembedResult` (already-completed work is preserved). Cancelled is not an error — it returns `Ok` with whatever was done.

#### Embedding deserialization note

Arrow deserialization reads vectors with priority 768 > 1024 > 1536 (first non-null wins). This means `get_all_memories()`/`get_all_facts()` always returns the smallest available dimension in `embedding`. The filter `embedding.len() != target_dim` correctly identifies records needing re-embed regardless of which column the old vector was stored in.

#### Startup auto-reembed removal

Delete the `tokio::spawn(reembed_all_facts(...))` block in `agent_init.rs` (lines ~451-462). Migration is now manual-only.

### 3. RPC Handler

#### Method: `memory.reembed`

**Request params** (all optional):
```json
{
    "target_dim": 768
}
```
Defaults to `embedder.dimensions()` if omitted.

**Sync response** (immediate, non-blocking):
```json
{
    "status": "started",
    "task_id": "reembed-1711036800000"
}
```

**Re-entrancy guard**: `AtomicBool` flag with RAII drop guard — the spawned task wraps execution in a guard struct whose `Drop` impl clears the flag, ensuring cleanup even on panic/error. If already running:
```json
{"error": {"code": -32001, "message": "Reembed already in progress"}}
```

**Cancellation**: RPC method `memory.reembed.cancel` sets the `cancel` AtomicBool, causing the background task to stop between batches. Returns `{"status": "cancelled"}`.

#### Background execution

Handler `tokio::spawn`s `reembed_all()` with a `watch::Sender`. A forwarding task reads from `watch::Receiver` and publishes to event_bus. Note: `watch` channel only retains the latest value, so rapid updates are coalesced — this is intentional for progress bars (only the latest state matters).

#### Event bus events

**Progress** — topic: `memory.reembed.progress`
```json
{
    "task_id": "reembed-1711036800000",
    "phase": "facts",
    "total": 150,
    "completed": 32,
    "failed": 1
}
```

**Completed** — topic: `memory.reembed.completed`
```json
{
    "task_id": "reembed-1711036800000",
    "facts_updated": 148,
    "facts_total": 150,
    "memories_updated": 320,
    "memories_total": 325,
    "errors": ["fact-abc: API timeout"]
}
```

#### Route registration

Register `"memory.reembed"` and `"memory.reembed.cancel"` in router, dependencies: `memory_db`, `embedder`, `event_bus`, shared `AtomicBool` (running flag), shared `Arc<AtomicBool>` (cancel token).

### 4. Panel Frontend

#### Location

Below the embedding provider settings section.

#### UI elements

1. **Migration button** — "迁移旧数据到当前 Provider", enabled by default
2. **Progress area** (shown after click):
   - Phase label: "正在处理 Facts..." / "正在处理 Memories..."
   - Progress bar: `completed / total`
   - Failure count (if any)
3. **Completion summary** (after migration ends):
   - "Facts: 148/150 已迁移, Memories: 320/325 已迁移"
   - Collapsible error list if errors exist
4. **Running state** — button disabled + "迁移中...", prevents re-trigger; show "取消" button to abort
5. **Error state** — if initial RPC call fails (no embedder configured, network error), show inline error message

#### Interaction flow

1. User clicks button → call `memory.reembed` RPC
2. Receive `{"status": "started"}` → show progress area
3. Subscribe `memory.reembed.progress` events → update progress bar in real-time
4. Receive `memory.reembed.completed` event → show summary, re-enable button

#### Principle

Panel is pure I/O (R4) — sends RPC, renders events, no business logic.

## Files to Modify

| File | Change |
|------|--------|
| `src/memory/store/mod.rs` | Add `get_all_memories()` to SessionStore trait |
| `src/memory/store/lance/sessions.rs` | Implement `get_all_memories()` |
| `src/memory/reembed.rs` | Rewrite: `reembed_all()` with progress, facts+memories |
| `src/bin/aleph/commands/start/builder/agent_init.rs` | Remove startup auto-reembed |
| `src/gateway/handlers/` | New handler module for `memory.reembed` |
| `src/gateway/router.rs` | Register `memory.reembed` route |
| `apps/panel/src/` | Migration button + progress UI in embedding settings |
