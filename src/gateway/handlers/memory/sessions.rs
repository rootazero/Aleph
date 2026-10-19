//! Memory Session Management Handlers
//!
//! RPC handlers for session management in Panel UI.

use serde::{Deserialize, Serialize};
use serde_json::json;

use std::collections::HashSet;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::types::SessionFilter;
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility;
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
    gateway_store: Arc<dyn SessionStore>,
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

    // Resolve the owner-visible session keys before applying pagination. The
    // Markdown projection owns agent/workspace filters, while the Gateway
    // session store is authoritative for authenticated user ownership.
    let visible_ids = match visible_session_ids(&gateway_store, params.agent_id.as_deref()).await {
        Ok(ids) => ids,
        Err(message) => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, message),
    };

    // A scoped caller needs all candidates before filtering; otherwise a
    // foreign row can consume a page slot and distort `has_more`.
    let fetch_limit = if visible_ids.is_some() {
        i64::MAX as usize
    } else {
        limit + 1
    };
    let fetch_offset = if visible_ids.is_some() {
        0
    } else {
        params.offset
    };

    let sessions = match store
        .list_sessions(
            params.agent_id.as_deref(),
            params.workspace_id.as_deref(),
            params.start_date,
            params.end_date,
            params.compression_status.as_deref(),
            fetch_limit,
            fetch_offset,
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

    let sessions: Vec<_> = sessions
        .into_iter()
        .filter(|session| {
            visible_ids
                .as_ref()
                .is_none_or(|ids| ids.contains(&session.session_id))
        })
        .collect();
    let total_visible = sessions.len();
    let page_start = if visible_ids.is_some() {
        params.offset.min(total_visible)
    } else {
        0
    };
    let has_more = if visible_ids.is_some() {
        total_visible.saturating_sub(page_start) > limit
    } else {
        sessions.len() > limit
    };
    let sessions: Vec<_> = sessions.into_iter().skip(page_start).take(limit).collect();
    let total = if visible_ids.is_some() {
        total_visible
    } else {
        sessions.len()
    };

    JsonRpcResponse::success(
        request.id,
        json!(ListSessionsResponse {
            sessions,
            total,
            has_more,
        }),
    )
}

fn session_id_is_safe(session_id: &str) -> bool {
    !session_id.is_empty()
        && !session_id.contains('/')
        && !session_id.contains('\\')
        && !std::path::Path::new(session_id).is_absolute()
        && std::path::Path::new(session_id).components().count() == 1
}

///
/// `None` means an internal/in-process caller with no caller identity and
/// preserves the existing unrestricted behavior. A scoped caller receives a
/// concrete allow-list from the authoritative Gateway session store.
async fn visible_session_ids(
    gateway_store: &Arc<dyn SessionStore>,
    agent_id: Option<&str>,
) -> Result<Option<HashSet<String>>, String> {
    let Some(owner) = visibility::visible_owner_filter() else {
        return Ok(None);
    };

    let sessions = gateway_store
        .list_sessions(SessionFilter {
            agent_id: agent_id.map(str::to_string),
            owner_visible_to: Some(owner),
            ..SessionFilter::default()
        })
        .await
        .map_err(|e| format!("Failed to resolve session ownership: {e}"))?;

    Ok(Some(
        sessions.into_iter().map(|session| session.key).collect(),
    ))
}

/// Check an addressed raw session against the authoritative Gateway owner.
async fn raw_session_visible(
    gateway_store: &Arc<dyn SessionStore>,
    session_id: &str,
    raw_metadata: &SessionMetadata,
) -> bool {
    if visibility::visible_owner_filter().is_none() {
        return true;
    }

    let Some(key) = SessionKey::parse(session_id) else {
        return false;
    };
    if key.agent_id() != raw_metadata.agent_id {
        return false;
    }

    match gateway_store.get_metadata(&key).await {
        Ok(Some(metadata)) => visibility::session_visible(&metadata),
        Ok(None) | Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::caller_identity::CALLER_USER;
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use crate::scope::{with_scope, ScopeAttribution};
    use tempfile::TempDir;

    fn test_store(temp: &TempDir) -> Arc<dyn SessionStore> {
        Arc::new(
            SessionManager::new(SessionManagerConfig {
                db_path: temp.path().join("sessions.db"),
                ..Default::default()
            })
            .expect("session store"),
        )
    }

    async fn seed(store: Arc<dyn SessionStore>, agent: &str, owner: &str) -> String {
        let key = SessionKey::main(agent);
        let id = key.to_key_string();
        with_scope(
            Some(ScopeAttribution::personal(owner)),
            store.get_or_create(&key),
        )
        .await
        .expect("seed session");
        id
    }

    #[tokio::test]
    async fn visible_session_ids_enforce_owner_and_preserve_internal_access() {
        let temp = TempDir::new().expect("temp dir");
        let store = test_store(&temp);
        let alice_id = seed(store.clone(), "alice", "u-alice").await;
        let bob_id = seed(store.clone(), "bob", "u-bob").await;

        let alice_visible = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                visible_session_ids(&store, None),
            )
            .await
            .expect("owner filter")
            .expect("scoped caller");
        assert_eq!(alice_visible, HashSet::from([alice_id.clone()]));
        assert!(!alice_visible.contains(&bob_id));

        let internal_visible = visible_session_ids(&store, None)
            .await
            .expect("internal access");
        assert!(internal_visible.is_none());

        let raw_metadata = SessionMetadata {
            session_id: alice_id.clone(),
            agent_id: "alice".to_string(),
            workspace_id: Some("workspace-a".to_string()),
            started_at: 0,
            ended_at: None,
            duration_seconds: None,
            transcript_path: "raw/sessions/alice/transcript.md".into(),
            tool_outputs_path: "raw/sessions/alice/tool_outputs.md".into(),
            message_count: 0,
            tool_call_count: 0,
            content_hash: None,
            compression_status: CompressionStatus::Pending,
            tags: Vec::new(),
        };
        assert!(
            CALLER_USER
                .scope(
                    Some("u-alice".to_string()),
                    raw_session_visible(&store, &alice_id, &raw_metadata),
                )
                .await
        );
        assert!(
            !CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    raw_session_visible(&store, &alice_id, &raw_metadata),
                )
                .await
        );
        let wrong_agent = SessionMetadata {
            agent_id: "bob".to_string(),
            ..raw_metadata
        };
        assert!(
            !CALLER_USER
                .scope(
                    Some("u-alice".to_string()),
                    raw_session_visible(&store, &alice_id, &wrong_agent),
                )
                .await
        );
    }

    #[test]
    fn session_ids_reject_path_traversal() {
        assert!(session_id_is_safe("agent:main:main"));
        assert!(!session_id_is_safe("../outside"));
        assert!(!session_id_is_safe("/tmp/outside"));
        assert!(!session_id_is_safe("agent/main"));
    }
}

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
    gateway_store: Arc<dyn SessionStore>,
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

    if !session_id_is_safe(&params.session_id) {
        return visibility::not_found_response(request.id);
    }

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

    if !raw_session_visible(&gateway_store, &params.session_id, &metadata).await {
        return visibility::not_found_response(request.id);
    }

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
    gateway_store: Arc<dyn SessionStore>,
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
            Ok(metadata)
                if raw_session_visible(&gateway_store, &search_result.session_id, &metadata)
                    .await =>
            {
                enriched_results.push(EnrichedSearchResult {
                    search_result,
                    metadata,
                });
            }
            Ok(_) => {
                // Foreign sessions are indistinguishable from absent rows.
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
    gateway_store: Arc<dyn SessionStore>,
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

    if !session_id_is_safe(&params.session_id) {
        return visibility::not_found_response(request.id);
    }
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
    if !raw_session_visible(&gateway_store, &params.session_id, &metadata).await {
        return visibility::not_found_response(request.id);
    }

    let status = CompressionStatus::from_label(&params.status);
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
