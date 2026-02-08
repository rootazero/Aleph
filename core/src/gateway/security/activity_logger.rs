// core/src/gateway/security/activity_logger.rs

//! Guest Activity Logger
//!
//! Manages activity logs for guest sessions with in-memory storage,
//! automatic cleanup, and query capabilities.

use super::activity_log::{
    ActivityLogQuery, ActivityLogQueryResult, ActivityStatus, ActivityType, GuestActivityLog,
};
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};

/// Maximum number of logs per session
const MAX_LOGS_PER_SESSION: usize = 1000;

/// How long to keep logs after session ends (milliseconds)
const LOG_RETENTION_MS: i64 = 3600_000; // 1 hour

/// Guest activity logger
///
/// Stores activity logs in memory with automatic cleanup and query support.
pub struct GuestActivityLogger {
    /// Logs by session ID
    logs: Arc<DashMap<String, Vec<GuestActivityLog>>>,
    /// Session end times (for cleanup)
    session_end_times: Arc<DashMap<String, i64>>,
}

impl GuestActivityLogger {
    /// Create a new activity logger
    pub fn new() -> Self {
        Self {
            logs: Arc::new(DashMap::new()),
            session_end_times: Arc::new(DashMap::new()),
        }
    }

    /// Log an activity
    pub fn log_activity(&self, log: GuestActivityLog) {
        let session_id = log.session_id.clone();

        debug!(
            session_id = %session_id,
            activity_type = ?log.activity_type,
            status = ?log.status,
            "Logging guest activity"
        );

        self.logs
            .entry(session_id.clone())
            .or_default()
            .push(log);

        // Enforce max logs limit
        if let Some(mut logs) = self.logs.get_mut(&session_id) {
            if logs.len() > MAX_LOGS_PER_SESSION {
                // Remove oldest logs
                let excess = logs.len() - MAX_LOGS_PER_SESSION;
                logs.drain(0..excess);
                debug!(
                    session_id = %session_id,
                    removed = excess,
                    "Removed excess logs"
                );
            }
        }
    }

    /// Log a tool call
    pub fn log_tool_call(
        &self,
        session_id: String,
        guest_id: String,
        tool_name: String,
        details: serde_json::Value,
        status: ActivityStatus,
        error: Option<String>,
    ) {
        let mut log = GuestActivityLog::new(
            session_id,
            guest_id,
            ActivityType::ToolCall { tool_name },
            details,
            status,
        );
        log.error = error;
        self.log_activity(log);
    }

    /// Log an RPC request
    pub fn log_rpc_request(
        &self,
        session_id: String,
        guest_id: String,
        method: String,
        details: serde_json::Value,
        status: ActivityStatus,
        error: Option<String>,
    ) {
        let mut log = GuestActivityLog::new(
            session_id,
            guest_id,
            ActivityType::RpcRequest { method },
            details,
            status,
        );
        log.error = error;
        self.log_activity(log);
    }

    /// Log a session event
    pub fn log_session_event(
        &self,
        session_id: String,
        guest_id: String,
        event: String,
        details: serde_json::Value,
    ) {
        let log = GuestActivityLog::new(
            session_id,
            guest_id,
            ActivityType::SessionEvent { event },
            details,
            ActivityStatus::Success,
        );
        self.log_activity(log);
    }

    /// Log a permission check
    pub fn log_permission_check(
        &self,
        session_id: String,
        guest_id: String,
        resource: String,
        allowed: bool,
        details: serde_json::Value,
    ) {
        let status = if allowed {
            ActivityStatus::Success
        } else {
            ActivityStatus::Failed
        };
        let log = GuestActivityLog::new(
            session_id,
            guest_id,
            ActivityType::PermissionCheck { resource },
            details,
            status,
        );
        self.log_activity(log);
    }

    /// Log an error
    pub fn log_error(
        &self,
        session_id: String,
        guest_id: String,
        error_type: String,
        error_message: String,
        details: serde_json::Value,
    ) {
        let log = GuestActivityLog::with_error(
            session_id,
            guest_id,
            ActivityType::Error { error_type },
            details,
            error_message,
        );
        self.log_activity(log);
    }

    /// Query activity logs for a session
    pub fn query_logs(
        &self,
        session_id: &str,
        query: &ActivityLogQuery,
    ) -> ActivityLogQueryResult {
        let logs = match self.logs.get(session_id) {
            Some(logs) => logs.clone(),
            None => {
                return ActivityLogQueryResult {
                    logs: vec![],
                    total: 0,
                    has_more: false,
                }
            }
        };

        // Filter logs
        let mut filtered: Vec<GuestActivityLog> = logs
            .iter()
            .filter(|log| query.matches(log))
            .cloned()
            .collect();

        // Sort by timestamp (newest first)
        filtered.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        let total = filtered.len();
        let offset = query.offset.unwrap_or(0);
        let limit = query.limit.unwrap_or(100);

        // Apply pagination
        let end = (offset + limit).min(total);
        let paginated = if offset < total {
            filtered[offset..end].to_vec()
        } else {
            vec![]
        };

        let has_more = end < total;

        ActivityLogQueryResult {
            logs: paginated,
            total,
            has_more,
        }
    }

    /// Get all logs for a session
    pub fn get_session_logs(&self, session_id: &str) -> Vec<GuestActivityLog> {
        self.logs
            .get(session_id)
            .map(|logs| logs.clone())
            .unwrap_or_default()
    }

    /// Mark session as ended (for cleanup tracking)
    pub fn mark_session_ended(&self, session_id: &str) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        self.session_end_times
            .insert(session_id.to_string(), now);

        debug!(session_id = %session_id, "Marked session as ended");
    }

    /// Clean up expired logs
    pub fn cleanup_expired_logs(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        let mut expired_sessions = Vec::new();

        // Find expired sessions
        for entry in self.session_end_times.iter() {
            let (session_id, end_time) = entry.pair();
            if now - end_time > LOG_RETENTION_MS {
                expired_sessions.push(session_id.clone());
            }
        }

        // Remove expired logs
        for session_id in expired_sessions {
            self.logs.remove(&session_id);
            self.session_end_times.remove(&session_id);
            debug!(session_id = %session_id, "Cleaned up expired logs");
        }
    }

    /// Get total number of logs across all sessions
    pub fn total_logs(&self) -> usize {
        self.logs.iter().map(|entry| entry.value().len()).sum()
    }

    /// Get number of active sessions (with logs)
    pub fn active_sessions(&self) -> usize {
        self.logs.len()
    }

    /// Clear all logs for a session
    pub fn clear_session_logs(&self, session_id: &str) {
        self.logs.remove(session_id);
        self.session_end_times.remove(session_id);
        debug!(session_id = %session_id, "Cleared session logs");
    }

    /// Clear all logs
    pub fn clear_all_logs(&self) {
        self.logs.clear();
        self.session_end_times.clear();
        warn!("Cleared all activity logs");
    }
}

impl Default for GuestActivityLogger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_log_activity() {
        let logger = GuestActivityLogger::new();

        logger.log_tool_call(
            "session-1".to_string(),
            "guest-1".to_string(),
            "translate".to_string(),
            json!({"input": "hello"}),
            ActivityStatus::Success,
            None,
        );

        let logs = logger.get_session_logs("session-1");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].session_id, "session-1");
    }

    #[test]
    fn test_max_logs_limit() {
        let logger = GuestActivityLogger::new();

        // Add more than MAX_LOGS_PER_SESSION logs
        for i in 0..(MAX_LOGS_PER_SESSION + 100) {
            logger.log_tool_call(
                "session-1".to_string(),
                "guest-1".to_string(),
                format!("tool-{}", i),
                json!({}),
                ActivityStatus::Success,
                None,
            );
        }

        let logs = logger.get_session_logs("session-1");
        assert_eq!(logs.len(), MAX_LOGS_PER_SESSION);
    }

    #[test]
    fn test_query_logs() {
        let logger = GuestActivityLogger::new();

        // Add some logs
        logger.log_tool_call(
            "session-1".to_string(),
            "guest-1".to_string(),
            "translate".to_string(),
            json!({}),
            ActivityStatus::Success,
            None,
        );

        logger.log_tool_call(
            "session-1".to_string(),
            "guest-1".to_string(),
            "summarize".to_string(),
            json!({}),
            ActivityStatus::Failed,
            Some("Error".to_string()),
        );

        // Query all logs
        let query = ActivityLogQuery::new();
        let result = logger.query_logs("session-1", &query);
        assert_eq!(result.total, 2);

        // Query only successful logs
        let query = ActivityLogQuery::new().with_status(ActivityStatus::Success);
        let result = logger.query_logs("session-1", &query);
        assert_eq!(result.total, 1);

        // Query with pagination
        let query = ActivityLogQuery::new().with_limit(1);
        let result = logger.query_logs("session-1", &query);
        assert_eq!(result.logs.len(), 1);
        assert!(result.has_more);
    }

    #[test]
    fn test_session_cleanup() {
        let logger = GuestActivityLogger::new();

        logger.log_tool_call(
            "session-1".to_string(),
            "guest-1".to_string(),
            "translate".to_string(),
            json!({}),
            ActivityStatus::Success,
            None,
        );

        logger.mark_session_ended("session-1");
        assert_eq!(logger.active_sessions(), 1);

        // Cleanup should not remove logs yet (within retention period)
        logger.cleanup_expired_logs();
        assert_eq!(logger.active_sessions(), 1);
    }

    #[test]
    fn test_clear_logs() {
        let logger = GuestActivityLogger::new();

        logger.log_tool_call(
            "session-1".to_string(),
            "guest-1".to_string(),
            "translate".to_string(),
            json!({}),
            ActivityStatus::Success,
            None,
        );

        logger.clear_session_logs("session-1");
        assert_eq!(logger.active_sessions(), 0);
    }
}
