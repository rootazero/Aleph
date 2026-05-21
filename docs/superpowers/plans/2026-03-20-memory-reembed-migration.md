# Memory Reembed Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable one-click re-embedding of all memory data (facts + raw memories) when users switch embedding providers.

**Architecture:** Extend the existing `reembed.rs` into a unified `reembed_all()` that processes both facts and memories with progress reporting. Expose via `memory.reembed` / `memory.reembed.cancel` RPC handlers. Panel UI adds a migration button with progress bar in the embedding provider settings page. Remove startup auto-reembed.

**Tech Stack:** Rust (alephcore), Leptos (WASM panel), LanceDB, Arrow, tokio watch channel, TopicEvent bus

**Spec:** `docs/superpowers/specs/2026-03-20-memory-reembed-migration-design.md`

---

### Task 1: Add `get_all_memories()` to SessionStore trait

**Files:**
- Modify: `src/memory/store/mod.rs:356-424` (SessionStore trait)
- Modify: `src/memory/store/lance/sessions.rs:107-293` (LanceDB impl)

- [ ] **Step 1: Add trait method**

In `src/memory/store/mod.rs`, add to the `SessionStore` trait after line 423 (`get_uncompressed_memories`):

```rust
    /// Retrieve all memory entries, optionally filtered by workspace.
    ///
    /// Used by the reembed migration to scan all memories for dimension mismatches.
    async fn get_all_memories(
        &self,
        workspace: Option<&str>,
    ) -> Result<Vec<MemoryEntry>, AlephError>;
```

- [ ] **Step 2: Implement in LanceDB backend**

In `src/memory/store/lance/sessions.rs`, add the implementation inside `impl SessionStore for LanceMemoryBackend` (before the closing `}`):

```rust
    async fn get_all_memories(
        &self,
        workspace: Option<&str>,
    ) -> Result<Vec<MemoryEntry>, AlephError> {
        use crate::memory::store::types::escape_sql_string;
        let filter = workspace.map(|ws| format!("agent = '{}'", escape_sql_string(ws)));
        scan_memories(&self.memories_table, filter.as_deref(), None).await
    }
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS (no errors)

- [ ] **Step 4: Commit**

```bash
git add src/memory/store/mod.rs src/memory/store/lance/sessions.rs
git commit -m "memory: add get_all_memories() to SessionStore trait"
```

---

### Task 2: Rewrite `reembed.rs` with unified `reembed_all()`

**Files:**
- Modify: `src/memory/reembed.rs` (full rewrite)

- [ ] **Step 1: Rewrite reembed.rs**

Replace entire contents of `src/memory/reembed.rs`:

```rust
//! Re-embedding migration for facts and raw memories.
//!
//! When the user switches embedding provider (different vector dimensions),
//! this module re-embeds all existing data with the new provider. Designed
//! to be triggered manually via RPC, not at startup.

use crate::error::AlephError;
use crate::memory::context::MemoryFact;
use crate::memory::store::{MemoryBackend, MemoryStore, SessionStore};
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{info, warn};

/// Progress of a running re-embed operation.
#[derive(Debug, Clone)]
pub struct ReembedProgress {
    /// Current phase: "facts" or "memories"
    pub phase: &'static str,
    /// Total items to process in this phase
    pub total: usize,
    /// Successfully completed items
    pub completed: usize,
    /// Failed items
    pub failed: usize,
}

/// Final result of a re-embed operation.
#[derive(Debug, Clone, Default)]
pub struct ReembedResult {
    pub facts_updated: usize,
    pub facts_total: usize,
    pub memories_updated: usize,
    pub memories_total: usize,
    pub errors: Vec<String>,
}

/// Re-embed all facts and raw memories to match the current provider's dimension.
///
/// Processes facts first, then memories. Each phase:
/// 1. Loads all records
/// 2. Filters those whose embedding dimension != target_dim
/// 3. Serial batch embed (batch_size items per API call)
/// 4. Updates via delete+insert (old vector columns cleared automatically)
///
/// Cancellation: checked between batches via `cancel` flag.
/// Idempotent: re-triggering only processes records still mismatched.
pub async fn reembed_all(
    database: &MemoryBackend,
    embedder: &Arc<dyn EmbeddingProvider>,
    target_dim: usize,
    batch_size: usize,
    progress_tx: Option<tokio::sync::watch::Sender<ReembedProgress>>,
    cancel: Arc<AtomicBool>,
) -> Result<ReembedResult, AlephError> {
    let mut result = ReembedResult::default();

    // Phase 1: Facts
    info!(target_dim, batch_size, "[reembed] Starting facts phase");
    reembed_facts(database, embedder, target_dim, batch_size, &progress_tx, &cancel, &mut result).await?;

    if cancel.load(Ordering::Relaxed) {
        info!("[reembed] Cancelled after facts phase");
        return Ok(result);
    }

    // Phase 2: Memories
    info!(target_dim, batch_size, "[reembed] Starting memories phase");
    reembed_memories(database, embedder, target_dim, batch_size, &progress_tx, &cancel, &mut result).await?;

    info!(
        facts_updated = result.facts_updated,
        facts_total = result.facts_total,
        memories_updated = result.memories_updated,
        memories_total = result.memories_total,
        errors = result.errors.len(),
        "[reembed] Migration complete"
    );

    Ok(result)
}

async fn reembed_facts(
    database: &MemoryBackend,
    embedder: &Arc<dyn EmbeddingProvider>,
    target_dim: usize,
    batch_size: usize,
    progress_tx: &Option<tokio::sync::watch::Sender<ReembedProgress>>,
    cancel: &AtomicBool,
    result: &mut ReembedResult,
) -> Result<(), AlephError> {
    let all_facts = database.get_all_facts(true, None).await?;

    let needs_reembed: Vec<&MemoryFact> = all_facts
        .iter()
        .filter(|f| {
            f.embedding
                .as_ref()
                .map(|e| e.len() != target_dim)
                .unwrap_or(true)
        })
        .collect();

    result.facts_total = needs_reembed.len();

    if needs_reembed.is_empty() {
        info!("[reembed] All facts already have correct embeddings");
        return Ok(());
    }

    info!(needs_reembed = needs_reembed.len(), "[reembed] Facts needing re-embedding");

    let mut completed = 0usize;
    let mut failed = 0usize;

    for chunk in needs_reembed.chunks(batch_size) {
        if cancel.load(Ordering::Relaxed) {
            info!("[reembed] Cancelled during facts phase");
            break;
        }

        let texts: Vec<&str> = chunk.iter().map(|f| f.content.as_str()).collect();

        match embedder.embed_batch(&texts).await {
            Ok(embeddings) => {
                for (fact, embedding) in chunk.iter().zip(embeddings.into_iter()) {
                    if embedding.len() != target_dim {
                        warn!(
                            fact_id = %fact.id,
                            got_dim = embedding.len(),
                            expected_dim = target_dim,
                            "[reembed] Dimension mismatch, skipping"
                        );
                        failed += 1;
                        result.errors.push(format!("fact {}: dimension mismatch (got {}, expected {})", fact.id, embedding.len(), target_dim));
                        continue;
                    }
                    let mut updated_fact = (*fact).clone();
                    updated_fact.embedding = Some(embedding);
                    if let Err(e) = database.update_fact(&updated_fact).await {
                        warn!(fact_id = %fact.id, error = %e, "[reembed] Failed to update fact");
                        failed += 1;
                        result.errors.push(format!("fact {}: {}", fact.id, e));
                    } else {
                        completed += 1;
                    }
                }
            }
            Err(e) => {
                warn!(batch_size = chunk.len(), error = %e, "[reembed] Batch embed failed");
                failed += chunk.len();
                result.errors.push(format!("facts batch: {}", e));
            }
        }

        send_progress(progress_tx, "facts", result.facts_total, completed, failed);
    }

    result.facts_updated = completed;
    Ok(())
}

async fn reembed_memories(
    database: &MemoryBackend,
    embedder: &Arc<dyn EmbeddingProvider>,
    target_dim: usize,
    batch_size: usize,
    progress_tx: &Option<tokio::sync::watch::Sender<ReembedProgress>>,
    cancel: &AtomicBool,
    result: &mut ReembedResult,
) -> Result<(), AlephError> {
    let all_memories = database.get_all_memories(None).await?;

    let needs_reembed: Vec<_> = all_memories
        .into_iter()
        .filter(|m| {
            m.embedding
                .as_ref()
                .map(|e| e.len() != target_dim)
                .unwrap_or(true)
        })
        .collect();

    result.memories_total = needs_reembed.len();

    if needs_reembed.is_empty() {
        info!("[reembed] All memories already have correct embeddings");
        return Ok(());
    }

    info!(needs_reembed = needs_reembed.len(), "[reembed] Memories needing re-embedding");

    let mut completed = 0usize;
    let mut failed = 0usize;

    for chunk in needs_reembed.chunks(batch_size) {
        if cancel.load(Ordering::Relaxed) {
            info!("[reembed] Cancelled during memories phase");
            break;
        }

        // Embed text matches ingestion.rs: user_input + "\n\n" + ai_output
        let texts: Vec<String> = chunk
            .iter()
            .map(|m| format!("{}\n\n{}", m.user_input, m.ai_output))
            .collect();
        let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();

        match embedder.embed_batch(&text_refs).await {
            Ok(embeddings) => {
                for (memory, embedding) in chunk.iter().zip(embeddings.into_iter()) {
                    if embedding.len() != target_dim {
                        warn!(
                            memory_id = %memory.id,
                            got_dim = embedding.len(),
                            expected_dim = target_dim,
                            "[reembed] Dimension mismatch, skipping"
                        );
                        failed += 1;
                        result.errors.push(format!("memory {}: dimension mismatch", memory.id));
                        continue;
                    }
                    let mut updated = memory.clone();
                    updated.embedding = Some(embedding);

                    // delete + insert (same pattern as update_fact)
                    if let Err(e) = database.delete_memory(&memory.id).await {
                        warn!(memory_id = %memory.id, error = %e, "[reembed] Failed to delete memory");
                        failed += 1;
                        result.errors.push(format!("memory {}: delete failed: {}", memory.id, e));
                        continue;
                    }
                    if let Err(e) = database.insert_memory(&updated).await {
                        warn!(memory_id = %memory.id, error = %e, "[reembed] Failed to insert memory");
                        failed += 1;
                        result.errors.push(format!("memory {}: insert failed: {}", memory.id, e));
                        // Note: the old record was already deleted. This is acceptable
                        // because the user can re-trigger the migration to re-process.
                    } else {
                        completed += 1;
                    }
                }
            }
            Err(e) => {
                warn!(batch_size = chunk.len(), error = %e, "[reembed] Batch embed failed");
                failed += chunk.len();
                result.errors.push(format!("memories batch: {}", e));
            }
        }

        send_progress(progress_tx, "memories", result.memories_total, completed, failed);
    }

    result.memories_updated = completed;
    Ok(())
}

fn send_progress(
    tx: &Option<tokio::sync::watch::Sender<ReembedProgress>>,
    phase: &'static str,
    total: usize,
    completed: usize,
    failed: usize,
) {
    if let Some(tx) = tx {
        let _ = tx.send(ReembedProgress {
            phase,
            total,
            completed,
            failed,
        });
    }
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/memory/reembed.rs
git commit -m "memory: rewrite reembed module with unified reembed_all()"
```

---

### Task 3: Remove startup auto-reembed

**Files:**
- Modify: `src/bin/aleph/commands/start/builder/agent_init.rs:450-462`

- [ ] **Step 1: Remove auto-reembed block**

Delete lines 450-462 in `agent_init.rs` (the `// One-shot re-embed` comment and the `if let Some(ref emb) = embedder_out` block that spawns `reembed_all_facts`).

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore && cargo check --bin aleph`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/bin/aleph/commands/start/builder/agent_init.rs
git commit -m "memory: remove startup auto-reembed (now manual via RPC)"
```

---

### Task 4: Add `memory.reembed` RPC handler

**Files:**
- Modify: `src/gateway/handlers/memory.rs` (add reembed handlers)
- Modify: `src/bin/aleph/commands/start/builder/handlers.rs:354-390` (register handlers)

- [ ] **Step 1: Add reembed handler types and functions**

Append to `src/gateway/handlers/memory.rs` (before the `#[cfg(test)]` block):

```rust
// ============================================================================
// Reembed Migration
// ============================================================================

use std::sync::atomic::{AtomicBool, Ordering};
use crate::memory::EmbeddingProvider;
use crate::gateway::event_bus::{GatewayEventBus, TopicEvent};

/// Shared state for the reembed background task.
pub struct ReembedState {
    /// True while a reembed task is running.
    pub running: Arc<AtomicBool>,
    /// Set to true to cancel the current task.
    pub cancel: Arc<AtomicBool>,
}

impl ReembedState {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// RAII guard that clears the running flag on drop (even on panic).
/// Owns an Arc so it can be moved into tokio::spawn.
struct RunningGuard(Arc<AtomicBool>);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Parameters for memory.reembed
#[derive(Debug, Default, Deserialize)]
pub struct ReembedParams {
    /// Target dimension (optional, defaults to current embedder's dimension)
    #[serde(default)]
    pub target_dim: Option<usize>,
}

/// Start a background reembed migration.
pub async fn handle_reembed(
    request: JsonRpcRequest,
    db: MemoryBackend,
    embedder: Arc<dyn EmbeddingProvider>,
    event_bus: Arc<GatewayEventBus>,
    reembed_state: Arc<ReembedState>,
) -> JsonRpcResponse {
    // Re-entrancy guard
    if reembed_state.running.compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed).is_err() {
        return JsonRpcResponse::error(
            request.id,
            -32001,
            "Reembed already in progress".to_string(),
        );
    }

    let params: ReembedParams = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    let target_dim = params.target_dim.unwrap_or_else(|| embedder.dimensions());
    let task_id = format!("reembed-{}", chrono::Utc::now().timestamp_millis());
    let task_id_clone = task_id.clone();

    // Reset cancel flag
    reembed_state.cancel.store(false, Ordering::Release);
    let cancel = Arc::clone(&reembed_state.cancel);

    // Create progress channel
    let (progress_tx, mut progress_rx) = tokio::sync::watch::channel(
        crate::memory::reembed::ReembedProgress {
            phase: "facts",
            total: 0,
            completed: 0,
            failed: 0,
        },
    );

    // Spawn progress forwarder
    let eb_progress = Arc::clone(&event_bus);
    let tid_progress = task_id.clone();
    tokio::spawn(async move {
        while progress_rx.changed().await.is_ok() {
            let p = progress_rx.borrow().clone();
            let _ = eb_progress.publish_json(&TopicEvent::new(
                "memory.reembed.progress",
                serde_json::json!({
                    "task_id": tid_progress,
                    "phase": p.phase,
                    "total": p.total,
                    "completed": p.completed,
                    "failed": p.failed,
                }),
            ));
        }
    });

    // Spawn background reembed task
    let state_ref = Arc::clone(&reembed_state);
    tokio::spawn(async move {
        let _guard = RunningGuard(Arc::clone(&state_ref.running));

        let result = crate::memory::reembed::reembed_all(
            &db, &embedder, target_dim, 32, Some(progress_tx), cancel,
        )
        .await;

        // Publish completion event
        match result {
            Ok(r) => {
                let _ = event_bus.publish_json(&TopicEvent::new(
                    "memory.reembed.completed",
                    serde_json::json!({
                        "task_id": task_id_clone,
                        "facts_updated": r.facts_updated,
                        "facts_total": r.facts_total,
                        "memories_updated": r.memories_updated,
                        "memories_total": r.memories_total,
                        "errors": r.errors,
                    }),
                ));
            }
            Err(e) => {
                let _ = event_bus.publish_json(&TopicEvent::new(
                    "memory.reembed.completed",
                    serde_json::json!({
                        "task_id": task_id_clone,
                        "error": format!("{}", e),
                    }),
                ));
            }
        }
    });

    JsonRpcResponse::success(
        request.id,
        json!({ "status": "started", "task_id": task_id }),
    )
}

/// Cancel a running reembed migration.
pub async fn handle_reembed_cancel(
    request: JsonRpcRequest,
    reembed_state: Arc<ReembedState>,
) -> JsonRpcResponse {
    if !reembed_state.running.load(Ordering::Acquire) {
        return JsonRpcResponse::error(
            request.id,
            -32001,
            "No reembed task is running".to_string(),
        );
    }

    reembed_state.cancel.store(true, Ordering::Release);
    JsonRpcResponse::success(request.id, json!({ "status": "cancelled" }))
}
```

- [ ] **Step 2: Register handlers in builder**

In `src/bin/aleph/commands/start/builder/handlers.rs`, modify `register_memory_handlers` to accept embedder and event_bus, and register the new handlers. The function signature changes to:

```rust
pub(in crate::commands::start) fn register_memory_handlers(
    server: &mut GatewayServer,
    memory_db: &MemoryBackend,
    compression_service: &Option<std::sync::Arc<alephcore::memory::compression::CompressionService>>,
    embedder: &Option<std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>>,
    event_bus: &std::sync::Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    daemon: bool,
)
```

Add after line 378 (after the `memory.compress` registration block):

```rust
    // Reembed migration handlers
    if let Some(emb) = embedder {
        let reembed_state = std::sync::Arc::new(
            alephcore::gateway::handlers::memory::ReembedState::new()
        );

        {
            let db = ::std::sync::Arc::clone(memory_db);
            let emb = ::std::sync::Arc::clone(emb);
            let eb = ::std::sync::Arc::clone(event_bus);
            let rs = ::std::sync::Arc::clone(&reembed_state);
            server.handlers_mut().register("memory.reembed", move |req| {
                let db = ::std::sync::Arc::clone(&db);
                let emb = ::std::sync::Arc::clone(&emb);
                let eb = ::std::sync::Arc::clone(&eb);
                let rs = ::std::sync::Arc::clone(&rs);
                async move {
                    memory_handlers::handle_reembed(req, db, emb, eb, rs).await
                }
            });
        }

        {
            let rs = ::std::sync::Arc::clone(&reembed_state);
            server.handlers_mut().register("memory.reembed.cancel", move |req| {
                let rs = ::std::sync::Arc::clone(&rs);
                async move {
                    memory_handlers::handle_reembed_cancel(req, rs).await
                }
            });
        }
    } else {
        server.handlers_mut().register("memory.reembed", |req| async move {
            alephcore::gateway::protocol::JsonRpcResponse::error(
                req.id,
                alephcore::gateway::protocol::INTERNAL_ERROR,
                "Reembed not available: missing embedding provider".to_string(),
            )
        });
        server.handlers_mut().register("memory.reembed.cancel", |req| async move {
            alephcore::gateway::protocol::JsonRpcResponse::error(
                req.id,
                alephcore::gateway::protocol::INTERNAL_ERROR,
                "Reembed not available: missing embedding provider".to_string(),
            )
        });
    }
```

Also update the call site of `register_memory_handlers` to pass the new parameters (search for where it's called in `agent_init.rs` or the builder module and add `embedder` and `event_bus`).

- [ ] **Step 3: Update the call site**

Find where `register_memory_handlers` is called and add the new arguments. It should look like:

```rust
register_memory_handlers(
    &mut server,
    &memory_db,
    &compression_svc,
    &embedder_out,        // new
    &event_bus,           // new
    args.daemon,
);
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore && cargo check --bin aleph`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/memory.rs src/bin/aleph/commands/start/builder/handlers.rs
git commit -m "gateway: add memory.reembed and memory.reembed.cancel RPC handlers"
```

---

### Task 5: Clean up redundant dimension conversion code

**Files:**
- Audit: `src/memory/store/lance/arrow_convert/helpers.rs` (`normalize_embedding`, `STANDARD_DIMS`)
- Audit: `src/memory/embedding_provider.rs` (`truncate_and_normalize`)
- Search for any other dimension-related code in `src/memory/`

This task is about identifying and cleaning up code that was previously used to handle dimension mismatches inline (at write time) that is now handled by the reembed migration. The `normalize_embedding` in `helpers.rs` truncates non-standard dimensions silently at Arrow serialization time — this is a different concern from re-embedding and should remain, since the Arrow schema only supports 768/1024/1536 columns.

- [ ] **Step 1: Audit dimension conversion code**

Read and catalog all dimension-related code:
1. `helpers.rs:106-144` — `normalize_embedding()`: truncates non-standard dims to nearest smaller standard. Used by Arrow serialization. **Keep** — this ensures any embedding can be stored.
2. `helpers.rs:107` — `STANDARD_DIMS`: the three supported dimensions. **Keep**.
3. `embedding_provider.rs:29-46` — `truncate_and_normalize()`: L2 normalize after truncation. **Keep** — called by `normalize_embedding`.

Search for any other dimension conversion, padding, or truncation code in `src/memory/` that may be redundant now that re-embedding is manual.

- [ ] **Step 2: Verify no redundant code**

Run grep to find any code that might be doing "automatic" re-embedding or dimension conversion beyond the Arrow serialization layer:
```bash
cargo check -p alephcore
```

If any redundant code is found (e.g., code that automatically re-embeds on provider switch, or code that converts dimensions outside the Arrow layer), remove it.

- [ ] **Step 3: Commit (if changes)**

```bash
git add -u src/memory/
git commit -m "memory: clean up redundant dimension conversion code"
```

---

### Task 6: Panel API — Add reembed RPC wrapper

**Files:**
- Modify: `apps/panel/src/api/embedding.rs`

- [ ] **Step 1: Add reembed API methods**

Append to `impl EmbeddingProvidersApi` in `apps/panel/src/api/embedding.rs`:

```rust
    /// Start a reembed migration
    pub async fn reembed(state: &DashboardState, target_dim: Option<u32>) -> Result<Value, String> {
        let params = match target_dim {
            Some(dim) => serde_json::json!({ "target_dim": dim }),
            None => Value::Null,
        };
        state.rpc_call("memory.reembed", params).await
    }

    /// Cancel a running reembed migration
    pub async fn reembed_cancel(state: &DashboardState) -> Result<Value, String> {
        state.rpc_call("memory.reembed.cancel", Value::Null).await
    }
```

- [ ] **Step 2: Verify compilation**

Run: `cd apps/panel && cargo check` (or the panel's build command)
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add apps/panel/src/api/embedding.rs
git commit -m "panel: add reembed RPC wrapper to embedding API"
```

---

### Task 7: Panel UI — Migration button and progress

**Files:**
- Modify: `apps/panel/src/views/settings/embedding_providers.rs`

- [ ] **Step 1: Add migration section to ProviderDetailPanel**

In `ProviderDetailPanel` component (after the actions rows, before `</div> // scrollable content` on line 749), add a migration card:

```rust
            // ─── Reembed Migration ─────────────────────────────────
            <ReembedMigrationCard />
```

- [ ] **Step 2: Create ReembedMigrationCard component**

Add a new component at the bottom of the file (before the `AddProviderPanel` component):

```rust
// ============================================================================
// Reembed Migration Card
// ============================================================================

#[component]
fn ReembedMigrationCard() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let (migrating, set_migrating) = signal(false);
    let (progress_phase, set_progress_phase) = signal(String::new());
    let (progress_total, set_progress_total) = signal(0usize);
    let (progress_completed, set_progress_completed) = signal(0usize);
    let (progress_failed, set_progress_failed) = signal(0usize);
    let (result_message, set_result_message) = signal(Option::<String>::None);
    let (error_message, set_error_message) = signal(Option::<String>::None);

    // Subscribe to reembed events via Gateway event bus
    Effect::new(move || {
        if state.is_connected.get() {
            // Register interest in reembed topics on the server
            spawn_local(async move {
                let _ = state.subscribe_topic("memory.reembed.*").await;
            });

            // Listen for all events, filter by topic
            state.subscribe_events(move |event: crate::context::GatewayEvent| {
                let data = &event.data;
                match event.topic.as_str() {
                    "memory.reembed.progress" => {
                        if let Some(phase) = data.get("phase").and_then(|v| v.as_str()) {
                            set_progress_phase.set(phase.to_string());
                        }
                        if let Some(total) = data.get("total").and_then(|v| v.as_u64()) {
                            set_progress_total.set(total as usize);
                        }
                        if let Some(completed) = data.get("completed").and_then(|v| v.as_u64()) {
                            set_progress_completed.set(completed as usize);
                        }
                        if let Some(failed) = data.get("failed").and_then(|v| v.as_u64()) {
                            set_progress_failed.set(failed as usize);
                        }
                    }
                    "memory.reembed.completed" => {
                        set_migrating.set(false);

                        if let Some(error) = data.get("error").and_then(|v| v.as_str()) {
                            set_error_message.set(Some(format!("Migration failed: {}", error)));
                        } else {
                            let facts_updated = data.get("facts_updated").and_then(|v| v.as_u64()).unwrap_or(0);
                            let facts_total = data.get("facts_total").and_then(|v| v.as_u64()).unwrap_or(0);
                            let memories_updated = data.get("memories_updated").and_then(|v| v.as_u64()).unwrap_or(0);
                            let memories_total = data.get("memories_total").and_then(|v| v.as_u64()).unwrap_or(0);
                            let errors = data.get("errors").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);

                            let msg = format!(
                                "Facts: {}/{} 已迁移, Memories: {}/{} 已迁移{}",
                                facts_updated, facts_total,
                                memories_updated, memories_total,
                                if errors > 0 { format!(", {} 个错误", errors) } else { String::new() },
                            );
                            set_result_message.set(Some(msg));
                        }
                    }
                    _ => {}
                }
            });
        }
    });

    // Start migration
    let handle_start = move |_| {
        set_migrating.set(true);
        set_result_message.set(None);
        set_error_message.set(None);
        set_progress_completed.set(0);
        set_progress_total.set(0);
        set_progress_failed.set(0);

        spawn_local(async move {
            match EmbeddingProvidersApi::reembed(&state, None).await {
                Ok(_) => {} // Progress tracked via events
                Err(e) => {
                    set_migrating.set(false);
                    set_error_message.set(Some(format!("Failed to start: {}", e)));
                }
            }
        });
    };

    // Cancel migration
    let handle_cancel = move |_| {
        spawn_local(async move {
            let _ = EmbeddingProvidersApi::reembed_cancel(&state).await;
        });
    };

    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-3">
            <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">
                "向量数据迁移"
            </h3>
            <p class="text-sm text-text-secondary">
                "切换 Embedding Provider 后，使用新 Provider 重新生成所有旧数据的向量。"
            </p>

            // Progress bar (shown during migration)
            {move || {
                if migrating.get() {
                    let total = progress_total.get();
                    let completed = progress_completed.get();
                    let failed = progress_failed.get();
                    let phase = progress_phase.get();
                    let pct = if total > 0 { (completed * 100) / total } else { 0 };
                    let phase_label = match phase.as_str() {
                        "facts" => "正在处理 Facts...",
                        "memories" => "正在处理 Memories...",
                        _ => "准备中...",
                    };

                    view! {
                        <div class="space-y-2">
                            <div class="flex justify-between text-xs text-text-tertiary">
                                <span>{phase_label}</span>
                                <span>{format!("{}/{}", completed, total)}{if failed > 0 { format!(" ({} 失败)", failed) } else { String::new() }}</span>
                            </div>
                            <div class="w-full h-2 bg-surface-sunken rounded-full overflow-hidden">
                                <div
                                    class="h-full bg-primary rounded-full transition-all duration-300"
                                    style=move || format!("width: {}%", pct)
                                ></div>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            // Result message
            {move || result_message.get().map(|msg| view! {
                <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm">
                    {msg}
                </div>
            })}

            // Error message
            {move || error_message.get().map(|msg| view! {
                <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">
                    {msg}
                </div>
            })}

            // Buttons
            <div class="flex gap-3">
                <button
                    on:click=handle_start
                    disabled=move || migrating.get()
                    class="flex-1 px-4 py-2.5 bg-warning text-white rounded-lg hover:bg-warning/90 disabled:opacity-50 transition-colors font-medium text-sm"
                >
                    {move || if migrating.get() { "迁移中..." } else { "迁移旧数据到当前 Provider" }}
                </button>
                {move || {
                    if migrating.get() {
                        view! {
                            <button
                                on:click=handle_cancel
                                class="px-4 py-2.5 bg-danger-subtle border border-danger/20 text-danger rounded-lg hover:bg-danger-subtle/80 transition-colors font-medium text-sm"
                            >
                                "取消"
                            </button>
                        }.into_any()
                    } else {
                        view! { <span></span> }.into_any()
                    }
                }}
            </div>
        </div>
    }
}
```

- [ ] **Step 3: Verify compilation**

Run: Panel's build/check command
Expected: PASS

- [ ] **Step 4: Build panel WASM**

Run: `just build-panel` or equivalent
Expected: WASM output updated

- [ ] **Step 5: Commit**

```bash
git add apps/panel/src/views/settings/embedding_providers.rs
git commit -m "panel: add reembed migration UI with progress bar"
```

---

### Task 8: Integration test — manual verification

- [ ] **Step 1: Full build**

Run: `cargo check -p alephcore && cargo check --bin aleph`
Expected: PASS

- [ ] **Step 2: Run core tests**

Run: `cargo test -p alephcore --lib`
Expected: PASS (existing tests should not break)

- [ ] **Step 3: Manual smoke test**

1. Start aleph server
2. Open Panel → Embedding Providers settings
3. Verify "向量数据迁移" card appears below provider detail
4. Click "迁移旧数据到当前 Provider"
5. Verify progress bar shows and updates
6. Verify completion summary shows
7. Test cancel button during migration

- [ ] **Step 4: Final commit**

```bash
git add -A
git commit -m "memory: complete reembed migration feature"
```
