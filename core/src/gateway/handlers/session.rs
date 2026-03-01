//! Session Handlers
//!
//! RPC handlers for session management: list, history, reset, send.
//!
//! Provides two sets of handlers:
//! - In-memory handlers using SessionStore (for development/testing)
//! - Database-backed handlers using SessionManager (for production)

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;
use tracing::debug;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS, INTERNAL_ERROR};
use super::super::router::SessionKey;
use super::super::session_manager::SessionManager;

/// Session information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    /// Session key string
    pub key: String,
    /// Agent ID
    pub agent_id: String,
    /// Session type (main, peer, task, ephemeral)
    pub session_type: String,
    /// Message count in session
    pub message_count: u32,
    /// Created timestamp (ISO 8601)
    pub created_at: String,
    /// Last activity timestamp (ISO 8601)
    pub last_active_at: String,
}

/// Session history message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessage {
    /// Message role (user, assistant, system)
    pub role: String,
    /// Message content
    pub content: String,
    /// Timestamp (ISO 8601)
    pub timestamp: String,
    /// Optional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

/// In-memory session store (simplified for Phase 2)
///
/// In a real implementation, this would use SQLite persistence.
pub struct SessionStore {
    sessions: Arc<RwLock<HashMap<String, SessionData>>>,
}

#[derive(Debug, Clone)]
struct SessionData {
    key: SessionKey,
    messages: Vec<HistoryMessage>,
    created_at: chrono::DateTime<chrono::Utc>,
    last_active_at: chrono::DateTime<chrono::Utc>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get or create a session
    pub async fn get_or_create(&self, key: &SessionKey) -> SessionInfo {
        let key_str = key.to_key_string();
        let mut sessions = self.sessions.write().await;

        let data = sessions.entry(key_str.clone()).or_insert_with(|| {
            let now = chrono::Utc::now();
            SessionData {
                key: key.clone(),
                messages: Vec::new(),
                created_at: now,
                last_active_at: now,
            }
        });

        self.data_to_info(&key_str, data)
    }

    /// List all sessions, optionally filtered by agent
    pub async fn list(&self, agent_id: Option<&str>) -> Vec<SessionInfo> {
        let sessions = self.sessions.read().await;

        sessions
            .iter()
            .filter(|(_, data)| {
                agent_id
                    .map(|id| data.key.agent_id() == id)
                    .unwrap_or(true)
            })
            .map(|(key, data)| self.data_to_info(key, data))
            .collect()
    }

    /// Get session history
    pub async fn get_history(
        &self,
        key: &str,
        limit: Option<usize>,
    ) -> Option<Vec<HistoryMessage>> {
        let sessions = self.sessions.read().await;

        sessions.get(key).map(|data| {
            let messages = &data.messages;
            match limit {
                Some(n) => messages.iter().rev().take(n).rev().cloned().collect(),
                None => messages.clone(),
            }
        })
    }

    /// Add a message to session history
    pub async fn add_message(&self, key: &str, role: &str, content: &str) {
        let mut sessions = self.sessions.write().await;

        if let Some(data) = sessions.get_mut(key) {
            data.messages.push(HistoryMessage {
                role: role.to_string(),
                content: content.to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                metadata: None,
            });
            data.last_active_at = chrono::Utc::now();
        }
    }

    /// Reset (clear) a session
    pub async fn reset(&self, key: &str) -> bool {
        let mut sessions = self.sessions.write().await;

        if let Some(data) = sessions.get_mut(key) {
            data.messages.clear();
            data.last_active_at = chrono::Utc::now();
            debug!("Reset session: {}", key);
            true
        } else {
            false
        }
    }

    /// Delete a session
    pub async fn delete(&self, key: &str) -> bool {
        let mut sessions = self.sessions.write().await;
        sessions.remove(key).is_some()
    }

    fn data_to_info(&self, key: &str, data: &SessionData) -> SessionInfo {
        let session_type = match &data.key {
            SessionKey::Main { .. } => "main",
            SessionKey::PerPeer { .. } => "peer",
            SessionKey::Task { .. } => "task",
            SessionKey::Ephemeral { .. } => "ephemeral",
        };

        SessionInfo {
            key: key.to_string(),
            agent_id: data.key.agent_id().to_string(),
            session_type: session_type.to_string(),
            message_count: data.messages.len() as u32,
            created_at: data.created_at.to_rfc3339(),
            last_active_at: data.last_active_at.to_rfc3339(),
        }
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle sessions.list RPC request
pub async fn handle_list(
    request: JsonRpcRequest,
    store: Arc<SessionStore>,
) -> JsonRpcResponse {
    let agent_id = request
        .params
        .as_ref()
        .and_then(|p| p.get("agent_id"))
        .and_then(|v| v.as_str());

    let sessions = store.list(agent_id).await;

    JsonRpcResponse::success(
        request.id,
        json!({
            "sessions": sessions,
            "count": sessions.len(),
        }),
    )
}

/// Handle sessions.history RPC request
pub async fn handle_history(
    request: JsonRpcRequest,
    store: Arc<SessionStore>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params object");
        }
    };

    let session_key = match params.get("session_key").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key");
        }
    };

    let limit = params
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);

    match store.get_history(session_key, limit).await {
        Some(messages) => JsonRpcResponse::success(
            request.id,
            json!({
                "session_key": session_key,
                "messages": messages,
                "count": messages.len(),
            }),
        ),
        None => JsonRpcResponse::error(request.id, INVALID_PARAMS, "Session not found"),
    }
}

/// Handle sessions.reset RPC request
pub async fn handle_reset(
    request: JsonRpcRequest,
    store: Arc<SessionStore>,
) -> JsonRpcResponse {
    let session_key = match &request.params {
        Some(Value::Object(map)) => map.get("session_key").and_then(|v| v.as_str()),
        _ => None,
    };

    match session_key {
        Some(key) => {
            let reset = store.reset(key).await;
            JsonRpcResponse::success(
                request.id,
                json!({
                    "session_key": key,
                    "reset": reset,
                }),
            )
        }
        None => JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key"),
    }
}

/// Handle sessions.delete RPC request
pub async fn handle_delete(
    request: JsonRpcRequest,
    store: Arc<SessionStore>,
) -> JsonRpcResponse {
    let session_key = match &request.params {
        Some(Value::Object(map)) => map.get("session_key").and_then(|v| v.as_str()),
        _ => None,
    };

    match session_key {
        Some(key) => {
            let deleted = store.delete(key).await;
            JsonRpcResponse::success(
                request.id,
                json!({
                    "session_key": key,
                    "deleted": deleted,
                }),
            )
        }
        None => JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key"),
    }
}

// =============================================================================
// Database-backed handlers using SessionManager
// =============================================================================

/// Handle sessions.list RPC request with database backend
pub async fn handle_list_db(
    request: JsonRpcRequest,
    manager: Arc<SessionManager>,
) -> JsonRpcResponse {
    let agent_id = request
        .params
        .as_ref()
        .and_then(|p| p.get("agent_id"))
        .and_then(|v| v.as_str());

    match manager.list_sessions(agent_id).await {
        Ok(sessions) => {
            let infos: Vec<SessionInfo> = sessions
                .into_iter()
                .map(|m| SessionInfo {
                    key: m.key,
                    agent_id: m.agent_id,
                    session_type: m.session_type,
                    message_count: m.message_count as u32,
                    created_at: chrono::DateTime::from_timestamp(m.created_at, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                    last_active_at: chrono::DateTime::from_timestamp(m.last_active_at, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                })
                .collect();
            let count = infos.len();
            JsonRpcResponse::success(
                request.id,
                json!({
                    "sessions": infos,
                    "count": count,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list sessions: {}", e),
        ),
    }
}

/// Handle sessions.history RPC request with database backend
pub async fn handle_history_db(
    request: JsonRpcRequest,
    manager: Arc<SessionManager>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params object");
        }
    };

    let session_key_str = match params.get("session_key").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key");
        }
    };

    let limit = params
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);

    // Parse session key from string
    let session_key = match SessionKey::from_key_string(session_key_str) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid session_key format");
        }
    };

    match manager.get_history(&session_key, limit).await {
        Ok(messages) => {
            let history: Vec<HistoryMessage> = messages
                .into_iter()
                .map(|m| HistoryMessage {
                    role: m.role,
                    content: m.content,
                    timestamp: chrono::DateTime::from_timestamp(m.timestamp, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                    metadata: m.metadata.map(|s| {
                        serde_json::from_str(&s).unwrap_or(Value::Null)
                    }),
                })
                .collect();
            let count = history.len();
            JsonRpcResponse::success(
                request.id,
                json!({
                    "session_key": session_key_str,
                    "messages": history,
                    "count": count,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get history: {}", e),
        ),
    }
}

/// Handle sessions.reset RPC request with database backend
pub async fn handle_reset_db(
    request: JsonRpcRequest,
    manager: Arc<SessionManager>,
) -> JsonRpcResponse {
    let session_key_str = match &request.params {
        Some(Value::Object(map)) => map.get("session_key").and_then(|v| v.as_str()),
        _ => None,
    };

    match session_key_str {
        Some(key_str) => {
            let session_key = match SessionKey::from_key_string(key_str) {
                Some(k) => k,
                None => {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        "Invalid session_key format",
                    );
                }
            };

            match manager.reset_session(&session_key).await {
                Ok(reset) => JsonRpcResponse::success(
                    request.id,
                    json!({
                        "session_key": key_str,
                        "reset": reset,
                    }),
                ),
                Err(e) => JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to reset session: {}", e),
                ),
            }
        }
        None => JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key"),
    }
}

/// Handle sessions.delete RPC request with database backend
pub async fn handle_delete_db(
    request: JsonRpcRequest,
    manager: Arc<SessionManager>,
) -> JsonRpcResponse {
    let session_key_str = match &request.params {
        Some(Value::Object(map)) => map.get("session_key").and_then(|v| v.as_str()),
        _ => None,
    };

    match session_key_str {
        Some(key_str) => {
            let session_key = match SessionKey::from_key_string(key_str) {
                Some(k) => k,
                None => {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        "Invalid session_key format",
                    );
                }
            };

            match manager.delete_session(&session_key).await {
                Ok(deleted) => JsonRpcResponse::success(
                    request.id,
                    json!({
                        "session_key": key_str,
                        "deleted": deleted,
                    }),
                ),
                Err(e) => JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to delete session: {}", e),
                ),
            }
        }
        None => JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_session_store_create() {
        let store = SessionStore::new();
        let key = SessionKey::main("main");

        let info = store.get_or_create(&key).await;
        assert_eq!(info.agent_id, "main");
        assert_eq!(info.session_type, "main");
        assert_eq!(info.message_count, 0);
    }

    #[tokio::test]
    async fn test_session_store_add_message() {
        let store = SessionStore::new();
        let key = SessionKey::main("main");
        let key_str = key.to_key_string();

        store.get_or_create(&key).await;
        store.add_message(&key_str, "user", "Hello").await;
        store.add_message(&key_str, "assistant", "Hi there!").await;

        let history = store.get_history(&key_str, None).await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, "user");
        assert_eq!(history[1].role, "assistant");
    }

    #[tokio::test]
    async fn test_session_store_reset() {
        let store = SessionStore::new();
        let key = SessionKey::main("main");
        let key_str = key.to_key_string();

        store.get_or_create(&key).await;
        store.add_message(&key_str, "user", "Hello").await;

        assert!(store.reset(&key_str).await);

        let history = store.get_history(&key_str, None).await.unwrap();
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn test_session_store_list() {
        let store = SessionStore::new();

        store.get_or_create(&SessionKey::main("main")).await;
        store.get_or_create(&SessionKey::main("work")).await;
        store.get_or_create(&SessionKey::peer("main", "window-1")).await;

        let all = store.list(None).await;
        assert_eq!(all.len(), 3);

        let main_only = store.list(Some("main")).await;
        assert_eq!(main_only.len(), 2);
    }

    #[tokio::test]
    async fn test_history_limit() {
        let store = SessionStore::new();
        let key = SessionKey::main("main");
        let key_str = key.to_key_string();

        store.get_or_create(&key).await;
        for i in 0..10 {
            store.add_message(&key_str, "user", &format!("Message {}", i)).await;
        }

        let limited = store.get_history(&key_str, Some(3)).await.unwrap();
        assert_eq!(limited.len(), 3);
        // Should get the last 3 messages
        assert!(limited[0].content.contains("7"));
        assert!(limited[1].content.contains("8"));
        assert!(limited[2].content.contains("9"));
    }
}
