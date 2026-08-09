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
///
/// The response carries `total`, the row count under the *same* `(agent_id,
/// query)` filter as `memories` (via [`MemoryBackend::count_raw_memories`]),
/// not the whole-store total. The panel's pager sizes itself from this field;
/// a store-wide total would leave "next" enabled past the last match once a
/// query narrows the list — the B4 phantom-page bug, resurrected for the
/// filtered case.
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

    // P1 partition isolation (spec §11-1c): a caller-supplied partition
    // suffix (`main__u-bob`) the caller does not own is invisible — same
    // empty-result shape as an unknown agent_id, no existence oracle. The
    // default (no suffix) always passes this check, so the common path is
    // unaffected.
    if !crate::gateway::visibility::partition_visible(agent_id) {
        return JsonRpcResponse::success(request.id, json!({ "memories": [], "total": 0 }));
    }

    let query = params
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty());

    let memories = match db.get_raw_memories_dashboard(
        Some(agent_id),
        query,
        params.limit as usize,
        params.offset as usize,
    ) {
        Ok(memories) => memories,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Search raw memories failed: {e}"),
            )
        }
    };

    // Same (agent_id, query) as the list above, so `total` describes exactly
    // that filtered set — not the whole store. A pager sized to the store
    // total would keep "next" enabled past the last match (B4 phantom-page,
    // resurrected for the filtered case).
    let total = match db.count_raw_memories(Some(agent_id), query) {
        Ok(total) => total,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Count raw memories failed: {e}"),
            )
        }
    };

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
    JsonRpcResponse::success(request.id, json!({ "memories": entries, "total": total }))
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
///
/// P1 partition isolation (spec §11-1c): this endpoint addresses a row by
/// bare `id` with no `agent_id` — unlike `memory.search`/`memory.listFacts`,
/// there is no caller-supplied partition to default or check directly. The
/// row's OWNING partition is resolved first (`raw_memory_agent_id`) and run
/// through `visibility::partition_visible` BEFORE the delete executes: an id
/// that doesn't exist and an id whose partition is invisible to the caller
/// get the exact same response (no oracle), and a denied delete never
/// touches the row.
pub async fn handle_delete(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    let params: DeleteParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let not_found = || {
        JsonRpcResponse::error(
            request.id.clone(),
            INTERNAL_ERROR,
            format!("No raw memory found with id '{}'", params.id),
        )
    };
    match db.raw_memory_agent_id(&params.id) {
        Ok(Some(owner)) if crate::gateway::visibility::partition_visible(&owner) => {}
        Ok(_) => return not_found(),
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Delete raw memory failed: {e}"),
            )
        }
    }

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

/// Fact entry for JSON serialization.
///
/// `tags` / `link_count` / `updated_at` are already carried by every
/// `NoteIndexEntry` the underlying query returns — this handler used to drop
/// them, leaving the panel with nothing per row but a filename.
#[derive(Debug, Clone, Serialize)]
pub struct FactEntry {
    pub id: String,
    pub agent_id: String,
    pub content: String,
    #[serde(rename = "fact_type")]
    pub note_type: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub category: String,
    pub path: String,
    pub tags: Vec<String>,
    pub link_count: usize,
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

    // P1 partition isolation: same "invisible partition reads as an unknown
    // agent" contract as `memory.search` above.
    if !crate::gateway::visibility::partition_visible(agent_id) {
        return JsonRpcResponse::success(request.id, json!({ "facts": [], "total": 0 }));
    }

    match db.list_notes(agent_id).await {
        Ok(notes) => {
            // `total` describes the whole agent store, so the pager can size
            // itself instead of guessing from a full page.
            let total = notes.len() as i64;
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
                    updated_at: n.updated_at,
                    category: n.category,
                    path: n.path,
                    tags: n.tags,
                    link_count: n.link_count,
                })
                .collect();

            JsonRpcResponse::success(request.id, json!({ "facts": entries, "total": total }))
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

/// Parameters for `memory.stats`.
#[derive(Debug, Default, Deserialize)]
pub struct StatsParams {
    /// Scope every count to one agent/partition. Omitted meaning depends on
    /// the caller (P1, spec §11-1c): an **unrestricted** caller (internal,
    /// cron, or an operator with no `CALLER_USER` scope) gets the whole
    /// store; a **member** gets the org partition
    /// ([`crate::routing::DEFAULT_AGENT_ID`]) instead — the whole-store
    /// rollup is never handed to a scoped caller just because they left this
    /// field off.
    #[serde(default)]
    pub agent_id: Option<String>,
}

/// Get memory statistics.
///
/// **Every count in one response shares one scope.** Mixing a cross-agent note
/// count with an agent-scoped list is what made the console's stat cards
/// contradict the rows beneath them, and what fed the raw pager a total that
/// did not describe the list it was paging.
///
/// The note graph is inherently per-agent, so an unscoped request returns
/// `null` for the graph counts rather than passing the default agent's graph
/// off as everyone's. A failed graph fetch for a *scoped* request also
/// returns `null`, not `0` — a failure to count is not "counted zero", and
/// padding it with a plausible-looking `0` would tell the panel something
/// false with total confidence.
///
/// P1 partition isolation (spec §11-1c): an explicit `agent_id` the caller
/// does not own reads as a real-but-empty agent (zero counts, not an error —
/// the same shape a genuinely unused agent id produces, so there is no
/// existence oracle). Omitting `agent_id` scopes a member to the org
/// partition rather than falling through to the whole-store rollup — see
/// [`StatsParams::agent_id`].
pub async fn handle_stats(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    use crate::memory::notes::store::NoteStore;

    let params: StatsParams = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    let agent: Option<String> = match params.agent_id {
        Some(requested) => {
            if !crate::gateway::visibility::partition_visible(&requested) {
                // Invisible partition: report the same "real, empty agent"
                // shape a never-used agent_id would produce, without ever
                // touching the store under the caller's chosen name.
                return JsonRpcResponse::success(
                    request.id,
                    json!({
                        "totalMemories": 0,
                        "totalFacts": 0,
                        "validFacts": 0,
                        "totalGraphNodes": 0,
                        "totalGraphEdges": 0,
                        "scope": "agent",
                    }),
                );
            }
            Some(requested)
        }
        // unrestricted caller (`None`): whole-store rollup, unchanged.
        None => crate::gateway::visibility::visible_owner_filter()
            .map(|_| crate::routing::DEFAULT_AGENT_ID.to_string()),
    };
    let agent = agent.as_deref();
    let scope = if agent.is_some() { "agent" } else { "global" };

    let raw_count = db.count_raw_memories(agent, None).unwrap_or(0);
    let note_count = match agent {
        Some(a) => db.count_notes(a).await.unwrap_or(0),
        None => db.count_all_notes().await.unwrap_or(0),
    };

    let (graph_nodes, graph_edges) = match agent {
        Some(a) => match db.get_graph_data(a, 10000).await {
            Ok((entries, links)) => (Some(entries.len() as i64), Some(links.len() as i64)),
            // A failed fetch is "we could not count", not "this agent has
            // zero nodes" — report the same `null` an unscoped request gets,
            // not a confident-looking zero.
            Err(_) => (None, None),
        },
        None => (None, None),
    };

    JsonRpcResponse::success(
        request.id,
        json!({
            "totalMemories": raw_count,
            "totalFacts": note_count,
            // Notes have no invalidated state (unlike the retired fact model),
            // so this mirrors totalFacts. Kept for response compatibility.
            "validFacts": note_count,
            "totalGraphNodes": graph_nodes,
            "totalGraphEdges": graph_edges,
            "scope": scope,
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
// List Corrections
// ============================================================================

/// Read-only listing of user corrections (raw `flag_user_correction` rows)
/// and their distillation status. Surfaces the correction→feedback lifecycle
/// to the panel; performs NO mutation (R7/R8: distillation stays LLM-driven).
///
/// P1 partition isolation (spec §11-1c): same caller-supplied `agent_id`
/// shape as `memory.search`, and the rows carry verbatim `content` (a
/// correction is something the user typed at the agent). An invisible
/// partition reads as an empty correction list — the same shape a partition
/// with no corrections produces.
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
    // P1 partition isolation — see this fn's doc. Before the watermark read,
    // so a denied caller learns nothing about the partition's dream state
    // either.
    if !crate::gateway::visibility::partition_visible(agent_id) {
        return JsonRpcResponse::success(request.id, json!({ "corrections": [] }));
    }

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

    // P1 partition isolation (spec §11-1c): same "invisible partition reads
    // as an unknown agent" contract as `memory.search`/`memory.listFacts` —
    // an empty evidence chain, not an error, and no store touch under the
    // caller's chosen name.
    if !crate::gateway::visibility::partition_visible(&agent) {
        use crate::builtin_tools::memory_trace::TraceResult;
        let empty = TraceResult {
            target: params.target,
            notes: Vec::new(),
            evidence: Vec::new(),
            write_decisions: Vec::new(),
        };
        return JsonRpcResponse::success(
            request.id,
            serde_json::to_value(empty).unwrap_or_default(),
        );
    }

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
            phase: "notes",
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

    /// P1 partition isolation: bob tracing alice's partition by name gets an
    /// empty evidence chain — the same shape an unused partition produces —
    /// not alice's real notes/evidence.
    #[tokio::test]
    async fn foreign_partition_traces_empty_not_the_owners_evidence() {
        use crate::gateway::caller_identity::CALLER_USER;

        let dir = tempfile::tempdir().unwrap();
        let db: crate::memory::store::MemoryBackend =
            Arc::new(SqliteMemoryBackend::new(&dir.path().join("m.db")).unwrap());

        let note = KnowledgeNote {
            title: "alice-secret".into(),
            category: "habits".into(),
            facts: vec!["daily running".into()],
            source_notes: vec!["raw-ev1".into()],
            ..Default::default()
        };
        db.index_note(&note, "main__u-alice", "habits")
            .await
            .unwrap();
        let mut raw = RawMemory::new("user: I run daily".into(), RawMemorySource::Transcript);
        raw.id = "raw-ev1".into();
        raw.agent_id = "main__u-alice".into();
        db.insert_raw_memory(&raw).await.unwrap();

        let req = JsonRpcRequest::with_id(
            "memory.trace",
            Some(
                json!({ "agent_id": "main__u-alice", "target": "habits/alice-secret", "kind": "note" }),
            ),
            json!(1),
        );
        let resp = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_trace(req, db).await
            })
            .await;
        assert!(resp.is_success(), "success, not an error: {:?}", resp.error);
        let result = resp.result.unwrap();
        assert!(result["notes"].as_array().unwrap().is_empty());
        assert!(result["evidence"].as_array().unwrap().is_empty());
    }
}

#[cfg(test)]
mod delete_tests {
    use super::*;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;

    fn db() -> MemoryBackend {
        let path = std::env::temp_dir().join(format!("mem_del_test_{}", uuid::Uuid::new_v4()));
        Arc::new(SqliteMemoryBackend::new(&path).unwrap())
    }

    fn req(id: &str) -> JsonRpcRequest {
        JsonRpcRequest::with_id("memory.delete", Some(json!({ "id": id })), json!(1))
    }

    async fn seed(db: &MemoryBackend, id: &str, agent_id: &str) {
        let mut raw = RawMemory::new("content".to_string(), RawMemorySource::Transcript);
        raw.id = id.to_string();
        raw.agent_id = agent_id.to_string();
        db.insert_raw_memory(&raw).await.unwrap();
    }

    #[tokio::test]
    async fn owner_can_delete_their_own_row() {
        let db = db();
        seed(&db, "r1", "main__u-alice").await;

        let resp = crate::gateway::caller_identity::CALLER_USER
            .scope(Some("u-alice".to_string()), async {
                handle_delete(req("r1"), db.clone()).await
            })
            .await;
        assert!(resp.is_success(), "{:?}", resp.error);
        assert_eq!(
            db.get_raws_by_ids("main__u-alice", &["r1".to_string()])
                .await
                .unwrap()
                .len(),
            0,
            "the row is actually gone"
        );
    }

    /// P1's own acceptance case: bob deleting alice's raw memory by its bare
    /// id — same "not found" response a genuinely missing id produces (no
    /// oracle), and the row is left completely intact.
    #[tokio::test]
    async fn foreign_partition_delete_is_denied_row_intact() {
        let db = db();
        seed(&db, "r1", "main__u-alice").await;

        let resp = crate::gateway::caller_identity::CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_delete(req("r1"), db.clone()).await
            })
            .await;
        assert!(resp.error.is_some(), "must be denied, not succeed");

        // Same response shape a genuinely unknown id produces — compared
        // against the SAME id string on a fresh, empty store, so any
        // difference can only come from the denial itself, not from the id
        // appearing in the message.
        let empty_db = self::db();
        let unknown_resp = crate::gateway::caller_identity::CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_delete(req("r1"), empty_db).await
            })
            .await;
        assert_eq!(
            resp.error.unwrap().message,
            unknown_resp.error.unwrap().message,
            "denied and genuinely-missing must be byte-identical (no oracle)"
        );

        // The row is intact — alice can still delete (and thus still read) it.
        let alice_resp = crate::gateway::caller_identity::CALLER_USER
            .scope(Some("u-alice".to_string()), async {
                handle_delete(req("r1"), db).await
            })
            .await;
        assert!(
            alice_resp.is_success(),
            "row must still exist for its real owner: {:?}",
            alice_resp.error
        );
    }

    #[tokio::test]
    async fn unknown_id_reports_not_found() {
        let db = db();
        let resp = handle_delete(req("nope"), db).await;
        assert!(resp.error.is_some());
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

    /// B4 phantom-page regression, filtered case: `total` must be the
    /// FILTERED row count (`count_raw_memories(agent, query)`), not the
    /// whole-store count and not the page size. Seed count (5) and match
    /// count (2) are deliberately different numbers so a store-total
    /// implementation cannot pass by coincidence, and the page is capped
    /// below the match count so `memories.len()` can't be mistaken for it
    /// either.
    #[tokio::test]
    async fn total_reflects_filtered_count_not_store_count() {
        let db = db();

        for (i, content) in [
            "apple pie recipe",
            "banana bread recipe",
            "apple crumble recipe",
            "cherry tart recipe",
            "date squares recipe",
        ]
        .into_iter()
        .enumerate()
        {
            let raw = RawMemory {
                id: format!("raw-{i}"),
                content: content.to_string(),
                source: RawMemorySource::Transcript,
                agent_id: "main".to_string(),
                session_id: None,
                path: None,
                attachment_text: None,
                is_processed: false,
                created_at: 1_700_000_000 + i as i64,
            };
            db.insert_raw_memory(&raw).await.unwrap();
        }

        let resp = handle_search(
            req(serde_json::json!({
                "agent_id": "main",
                "query": "apple",
                "limit": 1
            })),
            db.clone(),
        )
        .await;

        let result = resp.result.expect("success");
        assert_eq!(
            result["memories"].as_array().unwrap().len(),
            1,
            "page is capped by limit"
        );
        assert_eq!(
            result["total"], 2,
            "total must be the filtered count (2 rows contain 'apple'), not \
             the store total (5 seeded) and not the page size (1)"
        );

        // Empty query: total covers all (non-telemetry) rows.
        let resp = handle_search(
            req(serde_json::json!({
                "agent_id": "main",
                "query": "",
                "limit": 20
            })),
            db,
        )
        .await;
        assert_eq!(resp.result.expect("success")["total"], 5);
    }

    /// P1 partition isolation: alice's raw memories, addressed by their real
    /// partition id, are invisible to bob — same empty shape an unknown
    /// agent_id would produce (no existence oracle), not an error.
    #[tokio::test]
    async fn foreign_partition_reads_empty_not_the_owners_rows() {
        use crate::gateway::caller_identity::CALLER_USER;

        let db = db();
        let raw = RawMemory {
            id: "alice-secret".to_string(),
            content: "alice's private note".to_string(),
            source: RawMemorySource::Transcript,
            agent_id: "main__u-alice".to_string(),
            session_id: None,
            path: None,
            attachment_text: None,
            is_processed: false,
            created_at: 1_700_000_000,
        };
        db.insert_raw_memory(&raw).await.unwrap();

        // Sanity: the row is really there for its owner.
        let owner_resp = CALLER_USER
            .scope(Some("u-alice".to_string()), async {
                handle_search(
                    req(serde_json::json!({ "agent_id": "main__u-alice" })),
                    db.clone(),
                )
                .await
            })
            .await;
        assert_eq!(
            owner_resp.result.expect("success")["memories"]
                .as_array()
                .unwrap()
                .len(),
            1,
            "alice must see her own partition"
        );

        // Bob addresses the same partition by name — invisible.
        let bob_resp = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_search(req(serde_json::json!({ "agent_id": "main__u-alice" })), db).await
            })
            .await;
        let result = bob_resp.result.expect("success, not an error");
        assert!(
            result["memories"].as_array().unwrap().is_empty(),
            "bob must not see alice's partition"
        );
        assert_eq!(result["total"], 0);
    }

    /// Final-review I6: `memory.list_corrections` carried the same
    /// unenforced `agent_id` shape as `memory.search` above, and its rows
    /// carry verbatim `content` — things the user typed at the agent.
    #[tokio::test]
    async fn list_corrections_hides_a_foreign_partition() {
        use crate::gateway::caller_identity::CALLER_USER;

        let db = db();
        let correction = RawMemory {
            id: "alice-correction".to_string(),
            content: "no, my address is 12 Privacy Lane".to_string(),
            source: RawMemorySource::Correction {
                severity: "high".to_string(),
                suggested_rule: Some("remember the address".to_string()),
            },
            agent_id: "main__u-alice".to_string(),
            session_id: None,
            path: Some("aleph://correction/alice-correction".to_string()),
            attachment_text: None,
            is_processed: false,
            created_at: 1_700_000_000,
        };
        db.insert_raw_memory(&correction).await.unwrap();

        let ask = |caller: &'static str| {
            let db = db.clone();
            async move {
                CALLER_USER
                    .scope(Some(caller.to_string()), async {
                        handle_list_corrections(
                            req(serde_json::json!({ "agent_id": "main__u-alice" })),
                            db,
                        )
                        .await
                    })
                    .await
            }
        };

        // Sanity: the row is really there for its owner — otherwise the deny
        // assertion below would pass for the wrong reason.
        let owner = ask("u-alice").await;
        assert_eq!(
            owner.result.expect("success")["corrections"]
                .as_array()
                .unwrap()
                .len(),
            1,
            "alice must see her own correction"
        );

        let bob = ask("u-bob").await;
        assert!(
            bob.result.expect("success, not an error")["corrections"]
                .as_array()
                .unwrap()
                .is_empty(),
            "bob must not see alice's corrections"
        );
    }
}

#[cfg(test)]
mod stats_tests {
    use super::*;
    use crate::memory::notes::store::NoteStore;
    use crate::memory::notes::KnowledgeNote;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;

    fn db() -> MemoryBackend {
        let path = std::env::temp_dir().join(format!("mem_stats_test_{}", uuid::Uuid::new_v4()));
        Arc::new(SqliteMemoryBackend::new(&path).unwrap())
    }

    fn req(params: Option<serde_json::Value>) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "memory.stats".to_string(),
            params,
            id: Some(serde_json::json!(1)),
        }
    }

    fn note(title: &str) -> KnowledgeNote {
        KnowledgeNote {
            title: title.to_string(),
            category: "facts".to_string(),
            facts: vec!["f".to_string()],
            created_at: 1_700_000_000,
            updated_at: 1_700_000_000,
            content_hash: format!("h-{title}"),
            ..Default::default()
        }
    }

    fn raw(id: &str, agent: &str) -> RawMemory {
        RawMemory {
            id: id.to_string(),
            content: "c".to_string(),
            source: RawMemorySource::Transcript,
            agent_id: agent.to_string(),
            session_id: None,
            path: None,
            attachment_text: None,
            is_processed: false,
            created_at: 1_700_000_000,
        }
    }

    /// Two agents, asymmetric data. Scoped stats must describe ONE of them.
    async fn seed(db: &MemoryBackend) {
        db.index_note(&note("a1"), "alpha", "facts").await.unwrap();
        db.index_note(&note("a2"), "alpha", "facts").await.unwrap();
        db.index_note(&note("b1"), "beta", "facts").await.unwrap();
        db.insert_raw_memory(&raw("r1", "alpha")).await.unwrap();
        db.insert_raw_memory(&raw("r2", "alpha")).await.unwrap();
        db.insert_raw_memory(&raw("r3", "beta")).await.unwrap();
    }

    /// The regression: the stat cards used to show a cross-agent note count and
    /// a global raw count while the rows underneath were agent-scoped, so
    /// switching agents left the numbers describing a different population.
    #[tokio::test]
    async fn scoped_stats_describe_only_that_agent() {
        let db = db();
        seed(&db).await;

        let r = handle_stats(req(Some(serde_json::json!({ "agent_id": "alpha" }))), db).await;
        let v = r.result.expect("success");

        assert_eq!(v["scope"], "agent");
        assert_eq!(v["totalFacts"], 2, "alpha has 2 notes, not 3");
        assert_eq!(v["totalMemories"], 2, "alpha has 2 raw rows, not 3");
    }

    #[tokio::test]
    async fn unscoped_stats_are_global_and_disclaim_graph_counts() {
        let db = db();
        seed(&db).await;

        let r = handle_stats(req(None), db).await;
        let v = r.result.expect("success");

        assert_eq!(v["scope"], "global");
        assert_eq!(v["totalFacts"], 3, "all agents");
        assert_eq!(v["totalMemories"], 3, "all agents");
        // The note graph is inherently per-agent. Rather than silently report
        // the default agent's graph as if it were everyone's, an unscoped
        // request declines to answer.
        assert!(v["totalGraphNodes"].is_null());
        assert!(v["totalGraphEdges"].is_null());
    }

    #[tokio::test]
    async fn scoped_stats_answer_graph_counts() {
        let db = db();
        seed(&db).await;

        let r = handle_stats(req(Some(serde_json::json!({ "agent_id": "alpha" }))), db).await;
        let v = r.result.expect("success");
        assert_eq!(v["totalGraphNodes"], 2, "alpha's two notes are two nodes");
        assert!(v["totalGraphEdges"].is_i64());
    }

    /// `null` means "could not count", not "counted zero". An agent that
    /// genuinely has no notes must still get back a real `0`, not `null` —
    /// otherwise the fix that makes a *failed* graph fetch report `null`
    /// (see `handle_stats`) would also blur an empty-but-successful fetch
    /// into "unanswerable".
    #[tokio::test]
    async fn scoped_stats_zero_notes_reports_real_zero_not_null() {
        let db = db();
        seed(&db).await; // "gamma" is never seeded — zero notes, not an error

        let r = handle_stats(req(Some(serde_json::json!({ "agent_id": "gamma" }))), db).await;
        let v = r.result.expect("success");

        assert_eq!(v["scope"], "agent");
        assert_eq!(
            v["totalGraphNodes"], 0,
            "gamma has zero notes, but zero is a real, known count"
        );
        assert_eq!(v["totalGraphEdges"], 0);
        assert!(!v["totalGraphNodes"].is_null());
        assert!(!v["totalGraphEdges"].is_null());
    }

    /// P1: a member who omits `agent_id` is scoped to the org partition
    /// (`DEFAULT_AGENT_ID`, "main"), never the whole-store rollup — that
    /// rollup is reserved for unrestricted (internal/cron/operator-with-
    /// no-scope) callers, tested separately below.
    #[tokio::test]
    async fn member_omitted_agent_id_gets_org_partition_not_whole_store() {
        use crate::gateway::caller_identity::CALLER_USER;

        let db = db();
        seed(&db).await; // "alpha"/"beta" — neither is the org partition
        db.insert_raw_memory(&raw("r-main", "main")).await.unwrap();
        db.index_note(&note("main-note"), "main", "facts")
            .await
            .unwrap();

        let r = CALLER_USER
            .scope(Some("u-alice".to_string()), async {
                handle_stats(req(None), db).await
            })
            .await;
        let v = r.result.expect("success");

        assert_eq!(v["scope"], "agent", "member always gets a scoped answer");
        assert_eq!(v["totalMemories"], 1, "only the org (\"main\") row");
        assert_eq!(v["totalFacts"], 1, "only the org (\"main\") note");
    }

    /// The same omitted-`agent_id` request from an unrestricted caller (no
    /// `CALLER_USER` scope — internal/cron/legacy single-user) keeps the
    /// pre-P1 whole-store rollup, unchanged.
    #[tokio::test]
    async fn unrestricted_omitted_agent_id_still_gets_whole_store() {
        let db = db();
        seed(&db).await;

        let r = handle_stats(req(None), db).await;
        let v = r.result.expect("success");
        assert_eq!(v["scope"], "global");
        assert_eq!(v["totalFacts"], 3, "whole store, all agents");
    }

    /// Defense in depth: a member explicitly naming a foreign partition gets
    /// the same real-but-empty shape a genuinely unused agent id would
    /// produce — not the victim's real counts.
    #[tokio::test]
    async fn foreign_explicit_partition_reads_as_empty_not_the_owners_counts() {
        use crate::gateway::caller_identity::CALLER_USER;

        let db = db();
        db.insert_raw_memory(&raw("r1", "main__u-alice"))
            .await
            .unwrap();
        db.insert_raw_memory(&raw("r2", "main__u-alice"))
            .await
            .unwrap();

        let r = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_stats(
                    req(Some(serde_json::json!({ "agent_id": "main__u-alice" }))),
                    db,
                )
                .await
            })
            .await;
        let v = r.result.expect("success, not an error");
        assert_eq!(v["totalMemories"], 0, "not alice's real count of 2");
        assert_eq!(v["totalGraphNodes"], 0);
        assert_eq!(v["totalGraphEdges"], 0);
    }
}

#[cfg(test)]
mod list_facts_tests {
    use super::*;
    use crate::memory::notes::store::NoteStore;
    use crate::memory::notes::KnowledgeNote;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;

    fn db() -> MemoryBackend {
        let path = std::env::temp_dir().join(format!("mem_lf_test_{}", uuid::Uuid::new_v4()));
        Arc::new(SqliteMemoryBackend::new(&path).unwrap())
    }

    fn req(params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "memory.listFacts".to_string(),
            params: Some(params),
            id: Some(serde_json::json!(1)),
        }
    }

    #[tokio::test]
    async fn total_counts_the_whole_store_not_the_page() {
        let db = db();
        for i in 0..7 {
            let note = KnowledgeNote {
                title: format!("n{i}"),
                category: "facts".to_string(),
                facts: vec!["f".to_string()],
                created_at: 1_700_000_000,
                updated_at: 1_700_000_000,
                content_hash: format!("h{i}"),
                ..Default::default()
            };
            db.index_note(&note, "main", "facts").await.unwrap();
        }

        let v = handle_list_facts(
            req(serde_json::json!({ "agent_id": "main", "limit": 3, "offset": 0 })),
            db,
        )
        .await
        .result
        .expect("success");

        assert_eq!(v["facts"].as_array().unwrap().len(), 3, "page is capped");
        assert_eq!(v["total"], 7, "total describes the store, not the page");
    }

    /// tags / link_count / updated_at are already on every NoteIndexEntry the
    /// query returns. They used to be dropped here, which is why the panel had
    /// nothing to show per row beyond a filename.
    #[tokio::test]
    async fn passes_through_tags_link_count_and_updated_at() {
        let db = db();
        let mut note = KnowledgeNote {
            title: "tagged".to_string(),
            category: "facts".to_string(),
            facts: vec!["f".to_string()],
            created_at: 1_700_000_000,
            updated_at: 1_700_009_999,
            content_hash: "h".to_string(),
            ..Default::default()
        };
        note.tags = vec!["rust".to_string(), "ci".to_string()];
        db.index_note(&note, "main", "facts").await.unwrap();

        let v = handle_list_facts(
            req(serde_json::json!({ "agent_id": "main", "limit": 50, "offset": 0 })),
            db,
        )
        .await
        .result
        .expect("success");

        let row = &v["facts"][0];
        assert_eq!(row["updated_at"], 1_700_009_999_i64);
        let tags: Vec<String> = serde_json::from_value(row["tags"].clone()).unwrap();
        assert_eq!(tags, vec!["rust".to_string(), "ci".to_string()]);
        assert!(row["link_count"].is_u64());
    }

    /// P1 partition isolation, `listFacts` twin of `handle_search`'s test:
    /// a foreign partition reads as empty, not the owner's real facts.
    #[tokio::test]
    async fn foreign_partition_reads_empty_not_the_owners_facts() {
        use crate::gateway::caller_identity::CALLER_USER;

        let db = db();
        db.index_note(
            &KnowledgeNote {
                title: "alice-secret".to_string(),
                category: "facts".to_string(),
                facts: vec!["f".to_string()],
                created_at: 1_700_000_000,
                updated_at: 1_700_000_000,
                content_hash: "h".to_string(),
                ..Default::default()
            },
            "main__u-alice",
            "facts",
        )
        .await
        .unwrap();

        let r = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_list_facts(req(serde_json::json!({ "agent_id": "main__u-alice" })), db).await
            })
            .await;
        let v = r.result.expect("success, not an error");
        assert!(v["facts"].as_array().unwrap().is_empty());
        assert_eq!(v["total"], 0);
    }
}
