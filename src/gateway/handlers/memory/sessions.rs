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

    // Resolve the visibility constraint BEFORE the page is built. A scoped
    // caller scans the corpus in bounded storage batches and keeps only the
    // requested page, so the raw corpus is never materialized in memory
    // (H5-18). An unrestricted caller takes its page from the store and its
    // total from the store's own counter (H5-17).
    const SCAN_BATCH: usize = 100;
    let (sessions, total) = match &visible_ids {
        Some(ids) => {
            let mut page: Vec<SessionMetadata> = Vec::new();
            let mut visible_total = 0usize;
            let mut scan_offset = 0usize;
            loop {
                let batch = match store
                    .list_sessions(
                        params.agent_id.as_deref(),
                        params.workspace_id.as_deref(),
                        params.start_date,
                        params.end_date,
                        params.compression_status.as_deref(),
                        SCAN_BATCH,
                        scan_offset,
                    )
                    .await
                {
                    Ok(batch) => batch,
                    Err(e) => {
                        return JsonRpcResponse::error(
                            request.id,
                            INTERNAL_ERROR,
                            format!("Failed to list sessions: {}", e),
                        )
                    }
                };
                let batch_len = batch.len();
                for session in batch {
                    if !ids.contains(&session.session_id) {
                        continue;
                    }
                    if visible_total >= params.offset && page.len() < limit {
                        page.push(session);
                    }
                    visible_total += 1;
                }
                if batch_len < SCAN_BATCH {
                    break;
                }
                scan_offset += batch_len;
            }
            (page, visible_total)
        }
        None => {
            let page = match store
                .list_sessions(
                    params.agent_id.as_deref(),
                    params.workspace_id.as_deref(),
                    params.start_date,
                    params.end_date,
                    params.compression_status.as_deref(),
                    limit,
                    params.offset,
                )
                .await
            {
                Ok(page) => page,
                Err(e) => {
                    return JsonRpcResponse::error(
                        request.id,
                        INTERNAL_ERROR,
                        format!("Failed to list sessions: {}", e),
                    )
                }
            };
            let total = match store
                .count_sessions(
                    params.agent_id.as_deref(),
                    params.workspace_id.as_deref(),
                    params.start_date,
                    params.end_date,
                    params.compression_status.as_deref(),
                )
                .await
            {
                Ok(total) => total,
                Err(e) => {
                    return JsonRpcResponse::error(
                        request.id,
                        INTERNAL_ERROR,
                        format!("Failed to count sessions: {}", e),
                    )
                }
            };
            (page, total)
        }
    };
    // The visible rows start at `offset` in the store's ordering, so another
    // page exists exactly when more visible rows follow the current one.
    let has_more = total > params.offset.saturating_add(limit);

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

    #[tokio::test]
    async fn unrestricted_listing_total_is_independent_of_page() {
        let temp = TempDir::new().expect("temp dir");
        let store = Arc::new(MarkdownSessionStore::new(temp.path()).await.unwrap());
        let gateway = test_store(&temp);
        for index in 0..20 {
            let mut metadata = test_metadata(&format!("session-{index}"));
            metadata.started_at = index;
            store.create_session(metadata).await.unwrap();
        }
        // Each predicate independently excludes a row from the count and page.
        for predicate in 0..5 {
            let mut metadata = test_metadata(&format!("excluded-{predicate}"));
            metadata.started_at = 10;
            match predicate {
                0 => metadata.agent_id = "other".to_string(),
                1 => metadata.workspace_id = Some("other".to_string()),
                2 => metadata.started_at = -1,
                3 => metadata.started_at = 20,
                _ => {}
            }
            let id = metadata.session_id.clone();
            store.create_session(metadata).await.unwrap();
            if predicate == 4 {
                store
                    .update_compression_status(&id, CompressionStatus::Archived)
                    .await
                    .unwrap();
            }
        }
        for (offset, limit, expected_len, has_more) in [
            (0, 10, 10, true),
            (10, 10, 10, false),
            (30, 10, 0, false),
            (0, 0, 0, true),
        ] {
            let response = handle_list(
                JsonRpcRequest::new(
                    "memory.sessions.list",
                    Some(json!({
                        "agent_id": "agent", "workspace_id": "workspace",
                        "start_date": 0, "end_date": 19, "compression_status": "pending",
                        "offset": offset, "limit": limit,
                    })),
                    Some(json!(1)),
                ),
                store.clone(),
                gateway.clone(),
            )
            .await;
            assert!(response.error.is_none(), "{:?}", response.error);
            let result = response.result.unwrap();
            assert_eq!(result["total"], 20);
            assert_eq!(result["has_more"], has_more);
            assert_eq!(result["sessions"].as_array().unwrap().len(), expected_len);
        }
    }

    #[tokio::test]
    async fn scoped_listing_paginates_visible_rows_across_storage_batches() {
        let temp = TempDir::new().expect("temp dir");
        let store = Arc::new(MarkdownSessionStore::new(temp.path()).await.unwrap());
        let gateway = test_store(&temp);
        let newest = seed(gateway.clone(), "newest", "u-alice").await;
        let oldest = seed(gateway.clone(), "oldest", "u-alice").await;
        for index in 0..105 {
            let id = match index {
                0 => oldest.clone(),
                104 => newest.clone(),
                _ => format!("foreign-{index}"),
            };
            let mut metadata = test_metadata(&id);
            metadata.started_at = index;
            store.create_session(metadata).await.unwrap();
        }
        for (offset, limit, expected, has_more) in [
            (0, 1, vec![newest.as_str()], true),
            (1, 1, vec![oldest.as_str()], false),
            (2, 1, vec![], false),
            (0, 0, vec![], true),
        ] {
            let response = CALLER_USER
                .scope(
                    Some("u-alice".to_string()),
                    handle_list(
                        JsonRpcRequest::new(
                            "memory.sessions.list",
                            Some(json!({"offset": offset, "limit": limit})),
                            Some(json!(1)),
                        ),
                        store.clone(),
                        gateway.clone(),
                    ),
                )
                .await;
            assert!(response.error.is_none(), "{:?}", response.error);
            let result = response.result.unwrap();
            assert_eq!(result["total"], 2);
            assert_eq!(result["has_more"], has_more);
            let ids: Vec<_> = result["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|session| session["session_id"].as_str().unwrap())
                .collect();
            assert_eq!(ids, expected);
        }
    }

    fn test_metadata(session_id: &str) -> SessionMetadata {
        SessionMetadata {
            session_id: session_id.to_string(),
            agent_id: "agent".to_string(),
            workspace_id: Some("workspace".to_string()),
            started_at: 0,
            ended_at: None,
            duration_seconds: None,
            transcript_path: "transcript.md".into(),
            tool_outputs_path: "tool_outputs.md".into(),
            message_count: 0,
            tool_call_count: 0,
            content_hash: None,
            compression_status: CompressionStatus::Pending,
            tags: Vec::new(),
        }
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

    // Search transcripts with over-fetch to account for ownership filtering
    let fetch_limit = limit * 10;
    let search_results = match store
        .search_transcripts(&params.query, params.agent_id.as_deref(), fetch_limit)
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
        if enriched_results.len() >= limit {
            break;
        }
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

    let status = match params.status.as_str() {
        "pending" => CompressionStatus::Pending,
        "compressed" => CompressionStatus::Compressed,
        "archived" => CompressionStatus::Archived,
        _ => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid compression status: {}", params.status),
            )
        }
    };
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
