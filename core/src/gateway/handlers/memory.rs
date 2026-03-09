//! Memory RPC Handlers
//!
//! Handlers for memory management: search, delete, clear, stats, compression.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::parse_params;
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::memory::store::{MemoryBackend, MemoryStore, SessionStore};
use crate::gateway::workspace::WorkspaceFilter;
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

/// Search memories
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"memory.search","params":{"limit":10},"id":1}
/// ```
pub async fn handle_search(
    request: JsonRpcRequest,
    db: MemoryBackend,
) -> JsonRpcResponse {
    let params: SearchParams = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    let filter = crate::memory::store::types::MemoryFilter {
        window_title: params.window_title.clone(),
        workspace: params.agent_id.clone().map(WorkspaceFilter::Single),
        ..Default::default()
    };

    // Without a query embedding, fall back to recent memories
    match db
        .get_recent_memories(&filter, params.limit as usize)
        .await
    {
        Ok(memories) => {
            let entries: Vec<MemoryEntry> = memories
                .into_iter()
                .map(|m| MemoryEntry {
                    id: m.id,
                    agent_id: m.workspace,
                    window_title: m.context.window_title,
                    user_input: m.user_input,
                    ai_output: m.ai_output,
                    timestamp: m.context.timestamp,
                    similarity_score: m.similarity_score,
                })
                .collect();

            JsonRpcResponse::success(request.id, json!({ "memories": entries }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Search failed: {}", e),
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

/// Delete a single memory
pub async fn handle_delete(
    request: JsonRpcRequest,
    db: MemoryBackend,
) -> JsonRpcResponse {
    let params: DeleteParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match db.delete_memory(&params.id).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Delete failed: {}", e),
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

/// Clear memories (with optional filters)
pub async fn handle_clear(
    request: JsonRpcRequest,
    db: MemoryBackend,
) -> JsonRpcResponse {
    let params: ClearParams = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    match db
        .clear_memories(params.window_title.as_deref())
        .await
    {
        Ok(deleted_count) => {
            JsonRpcResponse::success(request.id, json!({ "deletedCount": deleted_count }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Clear failed: {}", e),
        ),
    }
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
    pub fact_type: String,
    pub confidence: f32,
    pub is_valid: bool,
    pub created_at: i64,
    pub category: String,
    pub path: String,
}

/// List compressed facts (Layer 2 data)
pub async fn handle_list_facts(
    request: JsonRpcRequest,
    db: MemoryBackend,
) -> JsonRpcResponse {
    let params: ListFactsParams = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    match db.get_all_facts(params.include_invalid, params.agent_id.as_deref()).await {
        Ok(mut facts) => {
            // Sort by created_at descending (most recent first)
            facts.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            facts.truncate(params.limit);

            let entries: Vec<FactEntry> = facts
                .into_iter()
                .map(|f| FactEntry {
                    id: f.id,
                    agent_id: f.workspace,
                    content: f.content,
                    fact_type: format!("{:?}", f.fact_type),
                    confidence: f.confidence,
                    is_valid: f.is_valid,
                    created_at: f.created_at,
                    category: format!("{:?}", f.category),
                    path: f.path,
                })
                .collect();

            JsonRpcResponse::success(request.id, json!({ "facts": entries }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("List facts failed: {}", e),
        ),
    }
}

// ============================================================================
// Clear Facts
// ============================================================================

/// Clear all compressed facts (Layer 2 data)
pub async fn handle_clear_facts(
    request: JsonRpcRequest,
    _db: MemoryBackend,
) -> JsonRpcResponse {
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
pub async fn handle_stats(
    request: JsonRpcRequest,
    db: MemoryBackend,
) -> JsonRpcResponse {
    match db.get_stats().await {
        Ok(stats) => JsonRpcResponse::success(
            request.id,
            json!({
                "totalMemories": stats.total_memories,
                "totalFacts": stats.total_facts,
                "validFacts": stats.valid_facts,
                "totalGraphNodes": stats.total_graph_nodes,
                "totalGraphEdges": stats.total_graph_edges,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Get stats failed: {}", e),
        ),
    }
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
pub async fn handle_app_list(
    request: JsonRpcRequest,
    _db: MemoryBackend,
) -> JsonRpcResponse {
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
