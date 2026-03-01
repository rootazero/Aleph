// core/src/gateway/security/guest_session_manager.rs

//! Guest Session Management
//!
//! Tracks active guest sessions, including connection time, last activity,
//! and tool usage statistics.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use crate::sync_primitives::Arc;
use thiserror::Error;

use aleph_protocol::GuestScope;

use super::activity_logger::GuestActivityLogger;

/// Guest session-related errors
#[derive(Debug, Error)]
pub enum GuestSessionError {
    #[error("Session not found")]
    SessionNotFound,
    #[error("Unauthorized")]
    Unauthorized,
}

/// Active guest session information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuestSession {
    /// Unique session ID
    pub session_id: String,
    /// Guest ID from invitation
    pub guest_id: String,
    /// Guest display name
    pub guest_name: String,
    /// Connection ID (WebSocket connection identifier)
    pub connection_id: String,
    /// Guest permissions scope
    pub scope: GuestScope,
    /// When the session started (Unix timestamp milliseconds)
    pub connected_at: i64,
    /// Last activity timestamp (Unix timestamp milliseconds)
    pub last_active_at: i64,
    /// Tools used during this session
    pub tools_used: Vec<String>,
    /// Number of requests made
    pub request_count: u32,
}

/// Manages active guest sessions
pub struct GuestSessionManager {
    /// Active sessions by session ID
    sessions: Arc<DashMap<String, GuestSession>>,
    /// Session ID lookup by connection ID
    connection_to_session: Arc<DashMap<String, String>>,
    /// Activity logger for tracking guest actions
    activity_logger: Arc<GuestActivityLogger>,
}

impl GuestSessionManager {
    /// Create a new guest session manager
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
            connection_to_session: Arc::new(DashMap::new()),
            activity_logger: Arc::new(GuestActivityLogger::new()),
        }
    }

    /// Create a new guest session manager with a shared activity logger
    pub fn with_logger(activity_logger: Arc<GuestActivityLogger>) -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
            connection_to_session: Arc::new(DashMap::new()),
            activity_logger,
        }
    }

    /// Get a reference to the activity logger
    pub fn activity_logger(&self) -> &Arc<GuestActivityLogger> {
        &self.activity_logger
    }

    /// Register a new guest session
    ///
    /// # Arguments
    /// * `session_id` - Unique session identifier
    /// * `guest_id` - Guest ID from invitation
    /// * `guest_name` - Guest display name
    /// * `connection_id` - WebSocket connection identifier
    /// * `scope` - Guest permissions scope
    ///
    /// # Returns
    /// The created guest session
    pub fn register_session(
        &self,
        session_id: String,
        guest_id: String,
        guest_name: String,
        connection_id: String,
        scope: GuestScope,
    ) -> GuestSession {
        let now = current_timestamp_ms();

        let session = GuestSession {
            session_id: session_id.clone(),
            guest_id: guest_id.clone(),
            guest_name: guest_name.clone(),
            connection_id: connection_id.clone(),
            scope,
            connected_at: now,
            last_active_at: now,
            tools_used: Vec::new(),
            request_count: 0,
        };

        self.sessions.insert(session_id.clone(), session.clone());
        self.connection_to_session
            .insert(connection_id.clone(), session_id.clone());

        // Log session connection event
        self.activity_logger.log_session_event(
            session_id.clone(),
            guest_id.clone(),
            "connected".to_string(),
            serde_json::json!({
                "guest_name": guest_name,
                "connection_id": connection_id,
            }),
        );

        session
    }

    /// Update last activity timestamp for a session
    pub fn update_activity(&self, session_id: &str) {
        if let Some(mut session) = self.sessions.get_mut(session_id) {
            session.last_active_at = current_timestamp_ms();
            session.request_count += 1;

            // Log RPC request activity
            self.activity_logger.log_rpc_request(
                session_id.to_string(),
                session.guest_id.clone(),
                "activity_update".to_string(),
                serde_json::json!({}),
                super::activity_log::ActivityStatus::Success,
                None,
            );
        }
    }

    /// Record tool usage for a session
    pub fn record_tool_usage(&self, session_id: &str, tool_name: String) {
        if let Some(mut session) = self.sessions.get_mut(session_id) {
            if !session.tools_used.contains(&tool_name) {
                session.tools_used.push(tool_name.clone());
            }

            // Log tool call activity
            self.activity_logger.log_tool_call(
                session_id.to_string(),
                session.guest_id.clone(),
                tool_name,
                serde_json::json!({}),
                super::activity_log::ActivityStatus::Success,
                None,
            );
        }
    }

    /// Get a session by session ID
    pub fn get_session(&self, session_id: &str) -> Option<GuestSession> {
        self.sessions.get(session_id).map(|s| s.clone())
    }

    /// Get a session by connection ID
    pub fn get_session_by_connection(&self, connection_id: &str) -> Option<GuestSession> {
        self.connection_to_session
            .get(connection_id)
            .and_then(|session_id| self.sessions.get(session_id.as_str()).map(|s| s.clone()))
    }

    /// List all active sessions
    pub fn list_sessions(&self) -> Vec<GuestSession> {
        self.sessions
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// List sessions for a specific guest
    pub fn list_sessions_for_guest(&self, guest_id: &str) -> Vec<GuestSession> {
        self.sessions
            .iter()
            .filter(|entry| entry.value().guest_id == guest_id)
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Terminate a session
    ///
    /// # Arguments
    /// * `session_id` - Session to terminate
    ///
    /// # Returns
    /// The terminated session, or error if not found
    pub fn terminate_session(&self, session_id: &str) -> Result<GuestSession, GuestSessionError> {
        let session = self
            .sessions
            .remove(session_id)
            .map(|(_, s)| s)
            .ok_or(GuestSessionError::SessionNotFound)?;

        // Remove connection mapping
        self.connection_to_session.remove(&session.connection_id);

        // Log session disconnection event
        self.activity_logger.log_session_event(
            session_id.to_string(),
            session.guest_id.clone(),
            "disconnected".to_string(),
            serde_json::json!({
                "duration_ms": current_timestamp_ms() - session.connected_at,
                "request_count": session.request_count,
                "tools_used": session.tools_used,
            }),
        );

        // Mark session as ended for log retention
        self.activity_logger.mark_session_ended(session_id);

        Ok(session)
    }

    /// Terminate session by connection ID (called on disconnect)
    pub fn terminate_by_connection(&self, connection_id: &str) -> Option<GuestSession> {
        let session_id = self.connection_to_session.remove(connection_id)?;
        let session = self.sessions.remove(&session_id.1).map(|(_, s)| s)?;

        // Log session disconnection event
        self.activity_logger.log_session_event(
            session_id.1.clone(),
            session.guest_id.clone(),
            "disconnected".to_string(),
            serde_json::json!({
                "duration_ms": current_timestamp_ms() - session.connected_at,
                "request_count": session.request_count,
                "tools_used": session.tools_used,
            }),
        );

        // Mark session as ended for log retention
        self.activity_logger.mark_session_ended(&session_id.1);

        Some(session)
    }

    /// Get session count
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Check if a connection has an active guest session
    pub fn has_session(&self, connection_id: &str) -> bool {
        self.connection_to_session.contains_key(connection_id)
    }
}

impl Default for GuestSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Get current timestamp in milliseconds
fn current_timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_get_session() {
        let manager = GuestSessionManager::new();

        let session = manager.register_session(
            "session1".to_string(),
            "guest1".to_string(),
            "Test Guest".to_string(),
            "conn1".to_string(),
            GuestScope {
                allowed_tools: vec!["translate".to_string()],
                expires_at: None,
                display_name: Some("Test".to_string()),
            },
        );

        assert_eq!(session.session_id, "session1");
        assert_eq!(session.guest_id, "guest1");

        let retrieved = manager.get_session("session1").unwrap();
        assert_eq!(retrieved.session_id, "session1");

        let by_conn = manager.get_session_by_connection("conn1").unwrap();
        assert_eq!(by_conn.session_id, "session1");
    }

    #[test]
    fn test_terminate_session() {
        let manager = GuestSessionManager::new();

        manager.register_session(
            "session1".to_string(),
            "guest1".to_string(),
            "Test Guest".to_string(),
            "conn1".to_string(),
            GuestScope {
                allowed_tools: vec![],
                expires_at: None,
                display_name: None,
            },
        );

        let terminated = manager.terminate_session("session1").unwrap();
        assert_eq!(terminated.session_id, "session1");

        assert!(manager.get_session("session1").is_none());
        assert!(manager.get_session_by_connection("conn1").is_none());
    }

    #[test]
    fn test_list_sessions() {
        let manager = GuestSessionManager::new();

        manager.register_session(
            "session1".to_string(),
            "guest1".to_string(),
            "Guest 1".to_string(),
            "conn1".to_string(),
            GuestScope {
                allowed_tools: vec![],
                expires_at: None,
                display_name: None,
            },
        );

        manager.register_session(
            "session2".to_string(),
            "guest1".to_string(),
            "Guest 1".to_string(),
            "conn2".to_string(),
            GuestScope {
                allowed_tools: vec![],
                expires_at: None,
                display_name: None,
            },
        );

        let sessions = manager.list_sessions();
        assert_eq!(sessions.len(), 2);

        let guest_sessions = manager.list_sessions_for_guest("guest1");
        assert_eq!(guest_sessions.len(), 2);
    }
}
