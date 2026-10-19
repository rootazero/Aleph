//! Memory Session Management Handlers
//!
//! RPC handlers for session management in Panel UI.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::memory::store::markdown::MarkdownSessionStore;
use crate::memory::store::session::{
    CompressionStatus, RawSessionStore, SessionMetadata, SessionSearchResult,
};
use crate::sync_primitives::Arc;

// ============================================================================
// memory.sessions.list
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct ListSessionsParams {
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub start_date: Option<i64>,
    #[serde(default)]
    pub end_date: Option<i64>,
    #[serde(default)]
    pub compression_status: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

const fn default_limit() -> usize {
    20
}

#[derive(Debug, Serialize)]
pub struct ListSessionsResponse {
    pub sessions: Vec<SessionMetadata>,
    pub total: usize,
    pub has_more: bool,
}

/// List sessions with pagination and filters.
///
/// **Method**: `memory.sessions.list`
///
/// **Parameters**:
/// - `agent_id` (optional): Filter by agent ID
/// - `workspace_id` (optional): Filter by workspace ID
/// - `start_date` (optional): Filter by start date (Unix timestamp)
/// - `end_date` (optional): Filter by end date (Unix timestamp)
/// - `compression_status` (optional): Filter by compression status (pending/compressed/archived)
/// - `limit` (optional): Maximum results (default: 20, max: 100)
/// - `offset` (optional): Number of rows to skip (default: 0)
///
/// **Returns**: Paginated list of sessions with `total` and `has_more`.
pub async fn handle_list(
    request: JsonRpcRequest,
    store: Arc<MarkdownSessionStore>,
) -> JsonRpcResponse {
    let params: ListSessionsParams = match request.params {
        Some(p) => match serde_json::from_value(p) {
            Ok(params) => params,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid parameters: {}", e),
                )
            }
        },
        None => ListSessionsParams {
            agent_id: None,
            workspace_id: None,
            start_date: None,
            end_date: None,
            compression_status: None,
            limit: default_limit(),
            offset: 0,
        },
    };

    // Enforce max limit
    let limit = params.limit.min(100);

    // List sessions
    let sessions = match store
        .list_sessions(
            params.agent_id.as_deref(),
            params.workspace_id.as_deref(),
            params.start_date,
            params.end_date,
            params.compression_status.as_deref(),
            limit + 1, // Fetch one extra to determine has_more
            params.offset,
        )
        .await
    {
        Ok(sessions) => sessions,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to list sessions: {}", e),
            )
        }
    };

    let has_more = sessions.len() > limit;
    let sessions: Vec<_> = sessions.into_iter().take(limit).collect();
    let total = sessions.len();

    JsonRpcResponse::success(
        request.id,
        json!(ListSessionsResponse {
            sessions,
            total,
            has_more,
        }),
    )
}

// ============================================================================
// memory.sessions.get
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct GetSessionParams {
    pub session_id: String,
}

#[derive(Debug, Serialize)]
pub struct GetSessionResponse {
    pub metadata: SessionMetadata,
    pub transcript: String,
    pub tool_outputs: String,
    pub attachments: Vec<crate::memory::store::session::AttachmentInfo>,
}

/// Get full session data (metadata, transcript, tool_outputs, attachments).
///
/// **Method**: `memory.sessions.get`
///
/// **Parameters**:
/// - `session_id`: Session ID to retrieve
///
/// **Returns**: Full session data including transcript and tool outputs.
pub async fn handle_get(
    request: JsonRpcRequest,
    store: Arc<MarkdownSessionStore>,
) -> JsonRpcResponse {
    let params: GetSessionParams = match request.params {
        Some(p) => match serde_json::from_value(p) {
            Ok(params) => params,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid parameters: {}", e),
                )
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing required parameter: session_id",
            )
        }
    };

    // Get metadata
    let metadata = match store.get_session_metadata(&params.session_id).await {
        Ok(metadata) => metadata,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to get session metadata: {}", e),
            )
        }
    };

    // Read transcript
    let transcript = match store.read_transcript(&params.session_id).await {
        Ok(transcript) => transcript,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to read transcript: {}", e),
            )
        }
    };

    // Read tool outputs
    let tool_outputs = match store.read_tool_outputs(&params.session_id).await {
        Ok(tool_outputs) => tool_outputs,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to read tool outputs: {}", e),
            )
        }
    };

    // List attachments
    let attachments = match store.list_attachments(&params.session_id).await {
        Ok(attachments) => attachments,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to list attachments: {}", e),
            )
        }
    };

    JsonRpcResponse::success(
        request.id,
        json!(GetSessionResponse {
            metadata,
            transcript,
            tool_outputs,
            attachments,
        }),
    )
}

// ============================================================================
// memory.sessions.search
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct SearchSessionsParams {
    pub query: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
}

const fn default_search_limit() -> usize {
    10
}

#[derive(Debug, Serialize)]
pub struct SearchSessionsResponse {
    pub results: Vec<EnrichedSearchResult>,
}

#[derive(Debug, Serialize)]
pub struct EnrichedSearchResult {
    #[serde(flatten)]
    pub search_result: SessionSearchResult,
    pub metadata: SessionMetadata,
}

/// Search across session transcripts using FTS5 index.
///
/// **Method**: `memory.sessions.search`
///
/// **Parameters**:
/// - `query`: Search query text
/// - `agent_id` (optional): Filter by agent ID
/// - `limit` (optional): Maximum results (default: 10, max: 50)
///
/// **Returns**: Search results with snippets, scores, and full metadata.
pub async fn handle_search(
    request: JsonRpcRequest,
    store: Arc<MarkdownSessionStore>,
) -> JsonRpcResponse {
    let params: SearchSessionsParams = match request.params {
        Some(p) => match serde_json::from_value(p) {
            Ok(params) => params,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid parameters: {}", e),
                )
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing required parameter: query",
            )
        }
    };

    // Enforce max limit
    let limit = params.limit.min(50);

    // Search transcripts
    let search_results = match store
        .search_transcripts(&params.query, params.agent_id.as_deref(), limit)
        .await
    {
        Ok(results) => results,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to search transcripts: {}", e),
            )
        }
    };

    // Enrich results with full metadata
    let mut enriched_results = Vec::new();
    for search_result in search_results {
        match store.get_session_metadata(&search_result.session_id).await {
            Ok(metadata) => {
                enriched_results.push(EnrichedSearchResult {
                    search_result,
                    metadata,
                });
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to get metadata for session {}: {}",
                    search_result.session_id,
                    e
                );
                // Continue with other results
            }
        }
    }

    JsonRpcResponse::success(
        request.id,
        json!(SearchSessionsResponse {
            results: enriched_results,
        }),
    )
}

// ============================================================================
// memory.sessions.update_compression
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct UpdateCompressionParams {
    pub session_id: String,
    pub status: String, // "pending" | "compressed" | "archived"
}

/// Update the compression status of a session.
///
/// **Method**: `memory.sessions.update_compression`
///
/// **Parameters**:
/// - `session_id`: Session ID
/// - `status`: New compression status (pending/compressed/archived)
///
/// **Returns**: Success confirmation.
pub async fn handle_update_compression(
    request: JsonRpcRequest,
    store: Arc<MarkdownSessionStore>,
) -> JsonRpcResponse {
    let params: UpdateCompressionParams = match request.params {
        Some(p) => match serde_json::from_value(p) {
            Ok(params) => params,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid parameters: {}", e),
                )
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing required parameters",
            )
        }
    };

    let status = CompressionStatus::from_str(&params.status);

    match store
        .update_compression_status(&params.session_id, status)
        .await
    {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "success": true })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to update compression status: {}", e),
        ),
    }
}
