//! Memory RPC Handlers
//!
//! Handlers for memory management: search, delete, clear, stats, compression.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use super::parse_params;
use crate::memory::store::MemoryBackend;
use crate::sync_primitives::Arc;

/// Memory entry for JSON serialization.
///
/// One raw conversation record. `user_input` / `ai_output` stay separate so the
/// panel can style the two halves independently — joining them into one string
/// server-side threw that away.
#[derive(Debug, Clone, Serialize)]
pub struct MemoryEntry {
    pub id: String,
    pub agent_id: String,
    pub window_title: String,
    pub user_input: String,
    pub ai_output: String,
    /// Session the row was recorded in, when known. Already selected by the
    /// dashboard query — previously dropped on the floor here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub timestamp: i64,
}

/// Window memory info
#[derive(Debug, Clone, Serialize)]
pub struct WindowMemoryInfo {
    pub window_title: String,
    pub memory_count: i64,
}

/// Memory statistics
#[derive(Debug, Clone, Serialize)]
pub struct MemoryStats {
    pub total_memories: i64,
    pub database_size_bytes: u64,
}

/// Compression statistics
#[derive(Debug, Clone, Serialize)]
pub struct CompressionStats {
    pub total_raw_memories: i64,
    pub total_facts: i64,
    pub valid_facts: i64,
}

/// Compression result
#[derive(Debug, Clone, Serialize)]
pub struct CompressionResult {
    pub memories_processed: i64,
    pub facts_extracted: i64,
    pub facts_invalidated: i64,
    pub duration_ms: u64,
}

// ============================================================================
// Search
// ============================================================================

/// Parameters for memory.search
#[derive(Debug, Deserialize)]
pub struct SearchParams {
    /// Search query text (optional - returns recent if empty)
    #[serde(default)]
    pub query: Option<String>,
    /// Filter by agent ID (workspace isolation)
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Filter by window title
    #[serde(default)]
    pub window_title: Option<String>,
    /// Maximum results (default: 20)
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// Number of rows to skip for pagination (browse mode only, default: 0)
    #[serde(default)]
    pub offset: u32,
}

const fn default_limit() -> u32 {
    20
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            query: None,
            agent_id: None,
            window_title: None,
            limit: default_limit(),
            offset: 0,
        }
    }
}

/// Search raw memory (Layer 1 conversation records).
///
/// `query` filters `content` by substring; empty `query` browses. This handler
/// is the **only** raw-memory entry point, and it returns **only** raw rows.
///
/// It used to run a note full-text search when `query` was non-empty —
/// duplicating `graph.search`, which calls the same `search_notes_fts`. The
/// panel wired that branch into its raw-memory table, so searching showed note
/// filenames dressed as conversation records and the row delete button targeted
/// `delete_raw_memory` with a note path (always `Ok(false)` → error → swallowed).
/// Note search belongs to `graph.search`; keep it there.
pub async fn handle_search(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    let params: SearchParams = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    // Scope to an agent namespace even when agent_id is omitted: passing None
    // drops the SQL `WHERE agent_id` clause and returns every agent's raw
    // memories, violating workspace isolation.
    let agent_id = params
        .agent_id
        .as_deref()
        .unwrap_or(crate::routing::DEFAULT_AGENT_ID);
    let query = params
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty());

    match db.get_raw_memories_dashboard(
        Some(agent_id),
        query,
        params.limit as usize,
        params.offset as usize,
    ) {
        Ok(memories) => {
            let entries: Vec<MemoryEntry> = memories
                .into_iter()
                .map(|m| MemoryEntry {
                    id: m.id,
                    agent_id: m.agent_id,
                    window_title: String::new(),
                    user_input: m.content,
                    ai_output: String::new(),
                    session_id: m.session_id,
                    timestamp: m.created_at,
                })
                .collect();
            JsonRpcResponse::success(request.id, json!({ "memories": entries }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Search raw memories failed: {e}"),
        ),
    }
}

// ============================================================================
// Delete
// ============================================================================

/// Parameters for memory.delete
#[derive(Debug, Deserialize)]
pub struct DeleteParams {
    /// Memory ID to delete
    pub id: String,
}

/// Delete a single raw memory entry (Layer 1 conversation record).
///
/// Raw memories live in the self-contained `raw_memories` table with no
/// vector/foreign-key linkage, so a single-row delete is safe. (Layer-2
/// knowledge notes are a separate model curated via the `note_manage` tool and
/// are not affected by this handler.)
pub async fn handle_delete(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    let params: DeleteParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match db.delete_raw_memory(&params.id) {
        Ok(true) => JsonRpcResponse::success(request.id, json!({ "ok": true })),
        Ok(false) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("No raw memory found with id '{}'", params.id),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Delete raw memory failed: {e}"),
        ),
    }
}

// ============================================================================
// Clear
// ============================================================================

/// Parameters for memory.clear
#[derive(Debug, Default, Deserialize)]
pub struct ClearParams {
    /// Filter by window title (optional)
    #[serde(default)]
    pub window_title: Option<String>,
}

/// Clear memories in bulk.
///
/// Bulk clearing is not a supported operation: raw conversation history lives
/// in the session store and knowledge notes are curated individually. The
/// handler previously returned `{ "deletedCount": 0 }`, which made
/// `aleph memory clear` print "All memory cleared" for a wipe that never ran.
pub async fn handle_clear(request: JsonRpcRequest, _db: MemoryBackend) -> JsonRpcResponse {
    let _params: ClearParams = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    JsonRpcResponse::error(
        request.id,
        INTERNAL_ERROR,
        "Bulk memory clearing is not supported in the notes-based memory model.".to_string(),
    )
}

// ============================================================================
// List Facts
// ============================================================================

/// Parameters for `memory.list_facts`
#[derive(Debug, Default, Deserialize)]
pub struct ListFactsParams {
    /// Filter by agent ID (workspace isolation)
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Maximum results (default: 50)
    #[serde(default = "default_facts_limit")]
    pub limit: usize,
    /// Number of rows to skip for pagination (default: 0)
    #[serde(default)]
    pub offset: usize,
    /// Include invalidated facts (default: false)
    #[serde(default)]
    pub include_invalid: bool,
}

const fn default_facts_limit() -> usize {
    50
}

/// Fact entry for JSON serialization
#[derive(Debug, Clone, Serialize)]
pub struct FactEntry {
    pub id: String,
    pub agent_id: String,
    pub content: String,
    #[serde(rename = "fact_type")]
    pub note_type: String,
    pub created_at: i64,
    pub category: String,
    pub path: String,
}

/// List note memories (compiled knowledge notes from `notes_index`).
pub async fn handle_list_facts(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    use crate::memory::notes::store::NoteStore;

    let params: ListFactsParams = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    let agent_id = params
        .agent_id
        .as_deref()
        .unwrap_or(crate::routing::DEFAULT_AGENT_ID);

    match db.list_notes(agent_id).await {
        Ok(notes) => {
            let entries: Vec<FactEntry> = notes
                .into_iter()
                .skip(params.offset)
                .take(params.limit)
                .map(|n| FactEntry {
                    id: n.path.clone(),
                    agent_id: n.agent_id,
                    content: n.filename.clone(),
                    note_type: n.category.clone(),
                    created_at: n.created_at,
                    category: n.category,
                    path: n.path,
                })
                .collect();

            JsonRpcResponse::success(request.id, json!({ "facts": entries }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("List notes failed: {e}"),
        ),
    }
}

// ============================================================================
// Clear Facts
// ============================================================================

/// Clear all knowledge notes.
///
/// Notes are not bulk-deletable through this RPC: they are curated
/// individually via the `note_manage` tool and decayed by the dream daemon.
/// The handler previously faked `{ "deletedCount": 0 }`, making
/// `aleph memory clear --facts-only` report a successful wipe that never ran.
pub async fn handle_clear_facts(request: JsonRpcRequest, _db: MemoryBackend) -> JsonRpcResponse {
    JsonRpcResponse::error(
        request.id,
        INTERNAL_ERROR,
        "Bulk note clearing is not supported; manage knowledge notes via the \
         note_manage tool."
            .to_string(),
    )
}

// ============================================================================
// Stats
// ============================================================================

/// Get memory statistics
pub async fn handle_stats(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    use crate::memory::notes::store::NoteStore;

    let raw_count = db.count_raw_memories(None, None).unwrap_or(0);

    // Note memory: count across all agents
    let note_count = db.count_all_notes().await.unwrap_or(0);

    // Graph stats for default agent
    let agent_id = crate::routing::DEFAULT_AGENT_ID;
    let (graph_nodes, graph_edges) = match db.get_graph_data(agent_id, 10000).await {
        Ok((entries, links)) => (entries.len() as i64, links.len() as i64),
        Err(_) => (0, 0),
    };

    JsonRpcResponse::success(
        request.id,
        json!({
            "totalMemories": raw_count,
            "totalFacts": note_count,
            "validFacts": note_count,
            "totalGraphNodes": graph_nodes,
            "totalGraphEdges": graph_edges,
        }),
    )
}

// ============================================================================
// Compress
// ============================================================================

/// Trigger memory compression
pub async fn handle_compress(
    request: JsonRpcRequest,
    service: Arc<crate::memory::compression::CompressionService>,
) -> JsonRpcResponse {
    match service.compress().await {
        Ok(result) => JsonRpcResponse::success(
            request.id,
            json!({
                "memoriesProcessed": result.memories_processed,
                "factsExtracted": result.facts_extracted,
                "factsInvalidated": result.facts_invalidated,
                "durationMs": result.duration_ms,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Compression failed: {e}"),
        ),
    }
}

// ============================================================================
// App List
// ============================================================================

/// List windows that have associated memories.
///
/// Window-anchored memory was a pre-notes-era concept; the notes-based model
/// has no per-window grouping, so this list is always empty. Kept as a
/// successful empty response for backward compatibility with older clients.
pub async fn handle_app_list(request: JsonRpcRequest, _db: MemoryBackend) -> JsonRpcResponse {
    let windows: Vec<WindowMemoryInfo> = Vec::new();
    JsonRpcResponse::success(request.id, json!({ "windows": windows }))
}

// ============================================================================
// List Corrections
// ============================================================================

/// Read-only listing of user corrections (raw `flag_user_correction` rows)
/// and their distillation status. Surfaces the correction→feedback lifecycle
/// to the panel; performs NO mutation (R7/R8: distillation stays LLM-driven).
pub async fn handle_list_corrections(
    request: JsonRpcRequest,
    db: MemoryBackend,
) -> JsonRpcResponse {
    use crate::memory::store::raw_memory::{RawMemorySource, RawMemoryStore};

    #[derive(serde::Deserialize, Default)]
    struct Params {
        agent_id: Option<String>,
        limit: Option<usize>,
        include_distilled: Option<bool>,
    }
    let params: Params = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    let agent_id = params
        .agent_id
        .as_deref()
        .unwrap_or(crate::routing::DEFAULT_AGENT_ID);
    let limit = params.limit.filter(|n| *n > 0).unwrap_or(50);
    let include_distilled = params.include_distilled.unwrap_or(true);

    // Distillation status comes from the FeedbackDistill watermark, NOT from
    // `is_processed`: that flag belongs to CompressionService's drain, and
    // `flag_user_correction`'s sedimentation kick sets it within seconds of
    // the correction landing — every row would show "distilled" long before
    // the dream stage actually consumed it. FeedbackDistill advances a
    // per-agent `created_at` watermark after each successfully distilled
    // batch (consumer key "feedback_distill" — keep in sync with
    // `memory::dreaming::stages::feedback_distill::WATERMARK_CONSUMER`), so a
    // correction is distilled exactly when `created_at <= watermark`.
    let watermark = db
        .get_dream_watermark("feedback_distill", agent_id)
        .unwrap_or_else(|e| {
            tracing::warn!(
                error = %e,
                "memory.list_corrections: failed to read feedback_distill watermark; treating as 0"
            );
            None
        })
        .unwrap_or(0);

    match db
        .get_raw_by_path_prefix("aleph://correction/", agent_id, limit)
        .await
    {
        Ok(rows) => {
            let corrections: Vec<_> = rows
                .into_iter()
                .filter(|r| include_distilled || r.created_at > watermark)
                .map(|r| {
                    let (severity, suggested_rule) = match &r.source {
                        RawMemorySource::Correction {
                            severity,
                            suggested_rule,
                        } => (severity.clone(), suggested_rule.clone()),
                        _ => ("low".to_string(), None),
                    };
                    let distilled = r.created_at <= watermark;
                    json!({
                        "id": r.id,
                        "content": r.content,
                        "severity": severity,
                        "suggested_rule": suggested_rule,
                        "status": if distilled { "distilled" } else { "pending" },
                        "created_at": r.created_at,
                    })
                })
                .collect();
            JsonRpcResponse::success(request.id, json!({ "corrections": corrections }))
        }
        Err(err) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("memory.list_corrections failed: {err}"),
        ),
    }
}

// ============================================================================
// Trace (evidence-chain walk)
// ============================================================================

/// Walk a memory claim down to ground-truth evidence.
///
/// Read-only; R4-compliant (I/O only). Forwards to
/// [`crate::builtin_tools::memory_trace::MemoryTraceTool::call_impl`].
pub async fn handle_trace(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    use crate::builtin_tools::memory_trace::{MemoryTraceArgs, MemoryTraceTool, TraceKind};

    #[derive(serde::Deserialize)]
    struct Params {
        agent_id: Option<String>,
        target: String,
        kind: TraceKind,
        #[serde(default)]
        max_results: Option<usize>,
    }

    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let agent = params
        .agent_id
        .unwrap_or_else(|| crate::routing::DEFAULT_AGENT_ID.to_string());

    let note_memory_dir = match crate::utils::paths::get_note_memory_dir() {
        Ok(d) => d,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("memory.trace: note dir: {e}"),
            )
        }
    };

    let tool = MemoryTraceTool::new(db, agent, note_memory_dir);
    match tool
        .call_impl(MemoryTraceArgs {
            target: params.target,
            kind: params.kind,
            max_results: params.max_results,
        })
        .await
    {
        Ok(res) => {
            JsonRpcResponse::success(request.id, serde_json::to_value(res).unwrap_or_default())
        }
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("memory.trace: {e}")),
    }
}

use crate::gateway::event_bus::{GatewayEventBus, TopicEvent};
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::{AtomicBool, Ordering};

/// Shared state for the reembed background task.
pub struct ReembedState {
    /// True while a reembed task is running.
    pub running: Arc<AtomicBool>,
    /// Set to true to cancel the current task.
    pub cancel: Arc<AtomicBool>,
}

impl Default for ReembedState {
    fn default() -> Self {
        Self::new()
    }
}

impl ReembedState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// RAII guard that clears the running flag on drop (even on panic).
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
    memory_dir: std::path::PathBuf,
    embedder: Arc<dyn EmbeddingProvider>,
    event_bus: Arc<GatewayEventBus>,
    reembed_state: Arc<ReembedState>,
) -> JsonRpcResponse {
    // Re-entrancy guard
    if reembed_state
        .running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
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
    let (progress_tx, mut progress_rx) =
        tokio::sync::watch::channel(crate::memory::reembed::ReembedProgress {
            phase: "facts",
            total: 0,
            completed: 0,
            failed: 0,
        });

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
            &db,
            &memory_dir,
            &embedder,
            target_dim,
            32,
            Some(progress_tx),
            cancel,
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

#[cfg(test)]
mod list_corrections_tests {
    use super::*;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;
    use serde_json::json;

    async fn seed(
        db: &SqliteMemoryBackend,
        id_suffix: &str,
        created_at: i64,
        processed: bool,
        sev: &str,
    ) {
        let mut raw = RawMemory::new(
            format!("correction {id_suffix}"),
            RawMemorySource::Correction {
                severity: sev.to_string(),
                suggested_rule: Some(format!("rule {id_suffix}")),
            },
        )
        .with_agent("main")
        .with_path(format!("aleph://correction/{id_suffix}"));
        raw.created_at = created_at;
        raw.is_processed = processed;
        db.insert_raw_memory(&raw).await.unwrap();
    }

    #[tokio::test]
    async fn maps_status_from_feedback_distill_watermark() {
        let backend = SqliteMemoryBackend::in_memory().unwrap();
        // c1 sits at/below the watermark → distilled; c2 sits above → pending
        // even though CompressionService already flipped its `is_processed`
        // flag (the flag that used to be misread as "distilled").
        seed(&backend, "c1", 1000, true, "high").await;
        seed(&backend, "c2", 2000, true, "low").await;
        backend
            .set_dream_watermark("feedback_distill", "main", 1500)
            .unwrap();
        let db: crate::memory::store::MemoryBackend = Arc::new(backend);

        let req = JsonRpcRequest::with_id(
            "memory.list_corrections",
            Some(json!({ "agent_id": "main" })),
            json!(1),
        );
        let resp = handle_list_corrections(req, db).await;
        assert!(resp.is_success(), "{:?}", resp.error);
        let items = resp.result.unwrap()["corrections"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(items.len(), 2);
        let c1 = items
            .iter()
            .find(|i| i["suggested_rule"] == "rule c1")
            .unwrap();
        assert_eq!(c1["status"], "distilled");
        assert_eq!(c1["severity"], "high");
        let c2 = items
            .iter()
            .find(|i| i["suggested_rule"] == "rule c2")
            .unwrap();
        assert_eq!(
            c2["status"], "pending",
            "above-watermark rows stay pending regardless of is_processed"
        );
    }

    #[tokio::test]
    async fn no_watermark_means_nothing_distilled() {
        let backend = SqliteMemoryBackend::in_memory().unwrap();
        // is_processed=true used to render this row "distilled" instantly
        // (sedimentation kicks within seconds); with no FeedbackDistill
        // watermark committed yet it must report pending.
        seed(&backend, "c1", 1000, true, "high").await;
        let db: crate::memory::store::MemoryBackend = Arc::new(backend);

        let req = JsonRpcRequest::with_id(
            "memory.list_corrections",
            Some(json!({ "agent_id": "main" })),
            json!(1),
        );
        let resp = handle_list_corrections(req, db).await;
        let items = resp.result.unwrap()["corrections"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["status"], "pending");
    }

    #[tokio::test]
    async fn include_distilled_false_filters_below_watermark() {
        let backend = SqliteMemoryBackend::in_memory().unwrap();
        seed(&backend, "c1", 1000, true, "high").await;
        seed(&backend, "c2", 2000, false, "low").await;
        backend
            .set_dream_watermark("feedback_distill", "main", 1500)
            .unwrap();
        let db: crate::memory::store::MemoryBackend = Arc::new(backend);

        let req = JsonRpcRequest::with_id(
            "memory.list_corrections",
            Some(json!({ "agent_id": "main", "include_distilled": false })),
            json!(1),
        );
        let resp = handle_list_corrections(req, db).await;
        let items = resp.result.unwrap()["corrections"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["status"], "pending");
        assert_eq!(items[0]["suggested_rule"], "rule c2");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_params_defaults() {
        let json = json!({});
        let params: SearchParams = serde_json::from_value(json).unwrap();
        assert!(params.query.is_none());
        assert!(params.agent_id.is_none());
        assert_eq!(params.limit, 20);
        assert_eq!(params.offset, 0);
    }

    #[test]
    fn test_search_params_offset_parsed() {
        let json = json!({ "limit": 50, "offset": 100 });
        let params: SearchParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.limit, 50);
        assert_eq!(params.offset, 100);
    }

    #[test]
    fn test_list_facts_params_offset_default() {
        let params: ListFactsParams = serde_json::from_value(json!({})).unwrap();
        assert_eq!(params.limit, 50);
        assert_eq!(params.offset, 0);
    }

    #[test]
    fn test_memory_entry_serialize() {
        let entry = MemoryEntry {
            id: "test-id".to_string(),
            agent_id: "main".to_string(),
            window_title: "Test Window".to_string(),
            user_input: "Hello".to_string(),
            ai_output: "Hi there".to_string(),
            session_id: Some("s-1".to_string()),
            timestamp: 1234567890,
        };

        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["id"], "test-id");
        assert_eq!(json["session_id"], "s-1");
    }

    #[test]
    fn test_memory_entry_no_session() {
        let entry = MemoryEntry {
            id: "test-id".to_string(),
            agent_id: "main".to_string(),
            window_title: "".to_string(),
            user_input: "".to_string(),
            ai_output: "".to_string(),
            session_id: None,
            timestamp: 0,
        };

        let json = serde_json::to_value(&entry).unwrap();
        assert!(json.get("session_id").is_none());
    }
}

#[cfg(test)]
mod trace_tests {
    use super::*;
    use crate::memory::notes::store::NoteStore;
    use crate::memory::notes::KnowledgeNote;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;
    use serde_json::json;

    #[tokio::test]
    async fn returns_notes_and_evidence_for_seeded_note() {
        let dir = tempfile::tempdir().unwrap();
        let db: crate::memory::store::MemoryBackend =
            Arc::new(SqliteMemoryBackend::new(&dir.path().join("m.db")).unwrap());

        // Seed a note that cites one raw memory (mirrors memory_trace.rs unit test).
        let note = KnowledgeNote {
            title: "exercise".into(),
            category: "habits".into(),
            facts: vec!["daily running".into()],
            source_notes: vec!["raw-ev1".into()],
            ..Default::default()
        };
        db.index_note(&note, "main", "habits").await.unwrap();

        // Insert the raw so evidence resolves (non-pruned).
        let mut raw = RawMemory::new("user: I run daily".into(), RawMemorySource::Transcript);
        raw.id = "raw-ev1".into();
        raw.agent_id = "main".into();
        db.insert_raw_memory(&raw).await.unwrap();

        let req = JsonRpcRequest::with_id(
            "memory.trace",
            Some(json!({ "agent_id": "main", "target": "habits/exercise", "kind": "note" })),
            json!(1),
        );
        let resp = handle_trace(req, db).await;
        assert!(resp.is_success(), "{:?}", resp.error);
        let result = resp.result.unwrap();
        assert!(
            result["notes"].as_array().is_some(),
            "response has notes array"
        );
        assert!(
            result["evidence"].as_array().is_some(),
            "response has evidence array"
        );
        let evidence = result["evidence"].as_array().unwrap();
        assert!(
            evidence.iter().any(|e| e["raw_id"] == "raw-ev1"),
            "evidence references seeded raw raw-ev1"
        );
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;
    use crate::memory::notes::store::NoteStore;
    use crate::memory::notes::KnowledgeNote;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;

    fn db() -> MemoryBackend {
        let path = std::env::temp_dir().join(format!("mem_search_test_{}", uuid::Uuid::new_v4()));
        Arc::new(SqliteMemoryBackend::new(&path).unwrap())
    }

    fn req(params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "memory.search".to_string(),
            params: Some(params),
            id: Some(serde_json::json!(1)),
        }
    }

    async fn seed(db: &MemoryBackend) {
        // One raw conversation row…
        let raw = RawMemory {
            id: "raw-1".to_string(),
            content: "we should run smoke tests before deploy".to_string(),
            source: RawMemorySource::Transcript,
            agent_id: "main".to_string(),
            session_id: Some("s-77".to_string()),
            path: None,
            layer: None,
            attachment_text: None,
            is_processed: false,
            created_at: 1_700_000_000,
        };
        db.insert_raw_memory(&raw).await.unwrap();

        // …and one note whose body ALSO contains the word "smoke", so an
        // accidental note-FTS branch would be visible in the assertion.
        let note = KnowledgeNote {
            title: "deploy-notes".to_string(),
            category: "facts".to_string(),
            facts: vec!["smoke".to_string()],
            created_at: 1_700_000_000,
            updated_at: 1_700_000_500,
            content_hash: "h1".to_string(),
            ..Default::default()
        };
        db.index_note(&note, "main", "facts").await.unwrap();
    }

    /// The core regression: a query must NEVER return note rows. The old
    /// handler ran a note FTS search here and returned note paths as if they
    /// were conversation records, so the console's "Raw" tab showed note
    /// filenames and its delete button targeted a table that does not hold them.
    #[tokio::test]
    async fn query_returns_raw_rows_never_notes() {
        let db = db();
        seed(&db).await;

        let resp = handle_search(
            req(serde_json::json!({
                "agent_id": "main",
                "query": "smoke",
                "limit": 20
            })),
            db,
        )
        .await;

        let memories = resp.result.expect("success")["memories"]
            .as_array()
            .expect("memories array")
            .clone();
        assert_eq!(memories.len(), 1, "only the raw row matches, not the note");
        assert_eq!(memories[0]["id"], "raw-1");
        assert_eq!(memories[0]["session_id"], "s-77");
        assert!(
            memories[0]["user_input"]
                .as_str()
                .unwrap()
                .contains("smoke tests"),
            "raw content must be returned verbatim, not a note filename"
        );
    }

    #[tokio::test]
    async fn empty_query_browses_all_raw_rows() {
        let db = db();
        seed(&db).await;

        let resp = handle_search(
            req(serde_json::json!({
                "agent_id": "main",
                "query": "",
                "limit": 20
            })),
            db,
        )
        .await;

        assert_eq!(
            resp.result.expect("success")["memories"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn query_with_no_match_returns_empty_not_error() {
        let db = db();
        seed(&db).await;

        let resp = handle_search(
            req(serde_json::json!({
                "agent_id": "main",
                "query": "zzz-nothing-matches",
                "limit": 20
            })),
            db,
        )
        .await;

        assert!(resp.error.is_none());
        assert!(resp.result.expect("success")["memories"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}
