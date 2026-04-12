//! Memory RPC Handlers
//!
//! Handlers for memory management: search, delete, clear, stats, compression.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use super::parse_params;
use crate::memory::store::MemoryBackend;
use crate::sync_primitives::Arc;

/// Memory entry for JSON serialization
#[derive(Debug, Clone, Serialize)]
pub struct MemoryEntry {
    pub id: String,
    pub agent_id: String,
    pub window_title: String,
    pub user_input: String,
    pub ai_output: String,
    pub timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity_score: Option<f32>,
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
}

fn default_limit() -> u32 {
    20
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            query: None,
            agent_id: None,
            window_title: None,
            limit: default_limit(),
        }
    }
}

/// Search raw memories (session summaries / conversation records).
///
/// Returns facts with `fact_source IN ('session_compressed', 'summary')`.
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"memory.search","params":{"limit":10},"id":1}
/// ```
pub async fn handle_search(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    let params: SearchParams = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    match db.get_raw_memories_dashboard(params.agent_id.as_deref(), params.limit as usize) {
        Ok(memories) => {
            let entries: Vec<MemoryEntry> = memories
                .into_iter()
                .map(|m| MemoryEntry {
                    id: m.id,
                    agent_id: m.agent_id,
                    window_title: String::new(),
                    user_input: m.content,
                    ai_output: String::new(),
                    timestamp: m.created_at,
                    similarity_score: None,
                })
                .collect();
            JsonRpcResponse::success(request.id, json!({ "memories": entries }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Search raw memories failed: {}", e),
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

/// Delete a single memory (no-op — raw memory storage removed)
pub async fn handle_delete(request: JsonRpcRequest, _db: MemoryBackend) -> JsonRpcResponse {
    let _params: DeleteParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Raw memory deletion removed — SessionStore no longer exists.
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
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

/// Clear memories (no-op — raw memory storage removed)
pub async fn handle_clear(request: JsonRpcRequest, _db: MemoryBackend) -> JsonRpcResponse {
    let _params: ClearParams = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    // Raw memory clearing removed — SessionStore no longer exists.
    JsonRpcResponse::success(request.id, json!({ "deletedCount": 0 }))
}

// ============================================================================
// List Facts
// ============================================================================

/// Parameters for memory.list_facts
#[derive(Debug, Default, Deserialize)]
pub struct ListFactsParams {
    /// Filter by agent ID (workspace isolation)
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Maximum results (default: 50)
    #[serde(default = "default_facts_limit")]
    pub limit: usize,
    /// Include invalidated facts (default: false)
    #[serde(default)]
    pub include_invalid: bool,
}

fn default_facts_limit() -> usize {
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
    pub confidence: f32,
    pub is_valid: bool,
    pub created_at: i64,
    pub category: String,
    pub path: String,
}

/// List note memories (compiled knowledge notes from notes_index).
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
                .take(params.limit)
                .map(|n| FactEntry {
                    id: n.path.clone(),
                    agent_id: n.agent_id,
                    content: n.filename.clone(),
                    note_type: n.category.clone(),
                    confidence: 1.0,
                    is_valid: true,
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
            format!("List notes failed: {}", e),
        ),
    }
}

// ============================================================================
// Clear Facts
// ============================================================================

/// Clear all compressed facts (Layer 2 data)
pub async fn handle_clear_facts(request: JsonRpcRequest, _db: MemoryBackend) -> JsonRpcResponse {
    // TODO: Implement clear_facts via new store API
    match Ok::<u64, crate::error::AlephError>(0) {
        Ok(deleted_count) => {
            JsonRpcResponse::success(request.id, json!({ "deletedCount": deleted_count }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Clear facts failed: {}", e),
        ),
    }
}

// ============================================================================
// Stats
// ============================================================================

/// Get memory statistics
pub async fn handle_stats(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    use crate::memory::notes::store::NoteStore;

    let raw_count = db.count_raw_memories().unwrap_or(0);

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
            format!("Compression failed: {}", e),
        ),
    }
}

// ============================================================================
// App List
// ============================================================================

/// Get list of windows with memories
pub async fn handle_app_list(request: JsonRpcRequest, _db: MemoryBackend) -> JsonRpcResponse {
    // TODO: Implement get_window_list via new store API
    match Ok::<Vec<(String, usize)>, crate::error::AlephError>(Vec::new()) {
        Ok(windows) => {
            let window_list: Vec<WindowMemoryInfo> = windows
                .into_iter()
                .map(|(window_title, memory_count)| WindowMemoryInfo {
                    window_title,
                    memory_count: memory_count as i64,
                })
                .collect();
            JsonRpcResponse::success(request.id, json!({ "windows": window_list }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Get app list failed: {}", e),
        ),
    }
}

// ============================================================================
// Reembed Migration
// ============================================================================

use crate::gateway::event_bus::{GatewayEventBus, TopicEvent};
use crate::memory::EmbeddingProvider;
use std::sync::atomic::{AtomicBool, Ordering};

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
mod tests {
    use super::*;

    #[test]
    fn test_search_params_defaults() {
        let json = json!({});
        let params: SearchParams = serde_json::from_value(json).unwrap();
        assert!(params.query.is_none());
        assert!(params.agent_id.is_none());
        assert_eq!(params.limit, 20);
    }

    #[test]
    fn test_memory_entry_serialize() {
        let entry = MemoryEntry {
            id: "test-id".to_string(),
            agent_id: "main".to_string(),
            window_title: "Test Window".to_string(),
            user_input: "Hello".to_string(),
            ai_output: "Hi there".to_string(),
            timestamp: 1234567890,
            similarity_score: Some(0.5), // Use 0.5 which can be represented exactly in f32
        };

        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["id"], "test-id");
        assert_eq!(json["similarity_score"], 0.5);
    }

    #[test]
    fn test_memory_entry_no_score() {
        let entry = MemoryEntry {
            id: "test-id".to_string(),
            agent_id: "main".to_string(),
            window_title: "".to_string(),
            user_input: "".to_string(),
            ai_output: "".to_string(),
            timestamp: 0,
            similarity_score: None,
        };

        let json = serde_json::to_value(&entry).unwrap();
        assert!(json.get("similarity_score").is_none());
    }
}
