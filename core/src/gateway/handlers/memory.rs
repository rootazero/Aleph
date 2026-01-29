//! Memory RPC Handlers
//!
//! Handlers for memory management: search, delete, clear, stats, compression.

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::memory::database::VectorDatabase;

/// Memory entry for JSON serialization
#[derive(Debug, Clone, Serialize)]
pub struct MemoryEntry {
    pub id: String,
    pub app_bundle_id: String,
    pub window_title: String,
    pub user_input: String,
    pub ai_output: String,
    pub timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity_score: Option<f32>,
}

/// App memory info
#[derive(Debug, Clone, Serialize)]
pub struct AppMemoryInfo {
    pub app_bundle_id: String,
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
    /// Filter by app bundle ID
    #[serde(default)]
    pub app_bundle_id: Option<String>,
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

/// Search memories
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"memory.search","params":{"limit":10},"id":1}
/// ```
pub async fn handle_search(
    request: JsonRpcRequest,
    memory_path: PathBuf,
) -> JsonRpcResponse {
    let params: SearchParams = match request.params {
        Some(ref p) => serde_json::from_value(p.clone()).unwrap_or(SearchParams {
            query: None,
            app_bundle_id: None,
            window_title: None,
            limit: default_limit(),
        }),
        None => SearchParams {
            query: None,
            app_bundle_id: None,
            window_title: None,
            limit: default_limit(),
        },
    };

    let db = match VectorDatabase::new(memory_path) {
        Ok(db) => db,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to open memory database: {}", e),
            );
        }
    };

    let app_filter = params.app_bundle_id.as_deref().unwrap_or("");
    let window_filter = params.window_title.as_deref().unwrap_or("");

    // Search with empty embedding returns recent memories filtered by context
    match db
        .search_memories(app_filter, window_filter, &[], params.limit)
        .await
    {
        Ok(memories) => {
            let entries: Vec<MemoryEntry> = memories
                .into_iter()
                .map(|m| MemoryEntry {
                    id: m.id,
                    app_bundle_id: m.context.app_bundle_id,
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
    memory_path: PathBuf,
) -> JsonRpcResponse {
    let params: DeleteParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: id required".to_string(),
            );
        }
    };

    let db = match VectorDatabase::new(memory_path) {
        Ok(db) => db,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to open memory database: {}", e),
            );
        }
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
#[derive(Debug, Deserialize)]
pub struct ClearParams {
    /// Filter by app bundle ID (optional)
    #[serde(default)]
    pub app_bundle_id: Option<String>,
    /// Filter by window title (optional)
    #[serde(default)]
    pub window_title: Option<String>,
}

/// Clear memories (with optional filters)
pub async fn handle_clear(
    request: JsonRpcRequest,
    memory_path: PathBuf,
) -> JsonRpcResponse {
    let params: ClearParams = match request.params {
        Some(ref p) => serde_json::from_value(p.clone()).unwrap_or(ClearParams {
            app_bundle_id: None,
            window_title: None,
        }),
        None => ClearParams {
            app_bundle_id: None,
            window_title: None,
        },
    };

    let db = match VectorDatabase::new(memory_path) {
        Ok(db) => db,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to open memory database: {}", e),
            );
        }
    };

    match db
        .clear_memories(params.app_bundle_id.as_deref(), params.window_title.as_deref())
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
// Clear Facts
// ============================================================================

/// Clear all compressed facts (Layer 2 data)
pub async fn handle_clear_facts(
    request: JsonRpcRequest,
    memory_path: PathBuf,
) -> JsonRpcResponse {
    let db = match VectorDatabase::new(memory_path) {
        Ok(db) => db,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to open memory database: {}", e),
            );
        }
    };

    match db.clear_facts().await {
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
    memory_path: PathBuf,
) -> JsonRpcResponse {
    let db = match VectorDatabase::new(memory_path) {
        Ok(db) => db,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to open memory database: {}", e),
            );
        }
    };

    match db.get_stats().await {
        Ok(stats) => JsonRpcResponse::success(
            request.id,
            json!({
                "totalMemories": stats.total_memories,
                "databaseSizeMb": stats.database_size_mb,
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
pub async fn handle_compress(request: JsonRpcRequest) -> JsonRpcResponse {
    // V2 compression is not yet fully implemented
    // Return a default result indicating no compression occurred
    JsonRpcResponse::success(
        request.id,
        json!(CompressionResult {
            memories_processed: 0,
            facts_extracted: 0,
            facts_invalidated: 0,
            duration_ms: 0,
        }),
    )
}

// ============================================================================
// App List
// ============================================================================

/// Get list of apps with memories
pub async fn handle_app_list(
    request: JsonRpcRequest,
    memory_path: PathBuf,
) -> JsonRpcResponse {
    let db = match VectorDatabase::new(memory_path) {
        Ok(db) => db,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to open memory database: {}", e),
            );
        }
    };

    match db.get_app_list().await {
        Ok(apps) => {
            let app_list: Vec<AppMemoryInfo> = apps
                .into_iter()
                .map(|(app_bundle_id, memory_count)| AppMemoryInfo {
                    app_bundle_id,
                    memory_count: memory_count as i64,
                })
                .collect();
            JsonRpcResponse::success(request.id, json!({ "apps": app_list }))
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
        assert!(params.app_bundle_id.is_none());
        assert_eq!(params.limit, 20);
    }

    #[test]
    fn test_memory_entry_serialize() {
        let entry = MemoryEntry {
            id: "test-id".to_string(),
            app_bundle_id: "com.example.app".to_string(),
            window_title: "Test Window".to_string(),
            user_input: "Hello".to_string(),
            ai_output: "Hi there".to_string(),
            timestamp: 1234567890,
            similarity_score: Some(0.95),
        };

        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["id"], "test-id");
        assert_eq!(json["similarityScore"], 0.95);
    }

    #[test]
    fn test_memory_entry_no_score() {
        let entry = MemoryEntry {
            id: "test-id".to_string(),
            app_bundle_id: "".to_string(),
            window_title: "".to_string(),
            user_input: "".to_string(),
            ai_output: "".to_string(),
            timestamp: 0,
            similarity_score: None,
        };

        let json = serde_json::to_value(&entry).unwrap();
        assert!(json.get("similarityScore").is_none());
    }
}
