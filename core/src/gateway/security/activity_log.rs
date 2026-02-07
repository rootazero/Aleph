// core/src/gateway/security/activity_log.rs

//! Guest Session Activity Logging
//!
//! Records and tracks all activities during guest sessions, including
//! tool calls, RPC requests, permission checks, and errors.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Guest session activity log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuestActivityLog {
    /// Unique log entry ID
    pub id: String,
    /// Session ID
    pub session_id: String,
    /// Guest ID
    pub guest_id: String,
    /// Activity type
    pub activity_type: ActivityType,
    /// Timestamp (Unix milliseconds)
    pub timestamp: i64,
    /// Activity details (JSON)
    pub details: serde_json::Value,
    /// Success/failure status
    pub status: ActivityStatus,
    /// Error message (if failed)
    pub error: Option<String>,
}

impl GuestActivityLog {
    /// Create a new activity log entry
    pub fn new(
        session_id: String,
        guest_id: String,
        activity_type: ActivityType,
        details: serde_json::Value,
        status: ActivityStatus,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            session_id,
            guest_id,
            activity_type,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
            details,
            status,
            error: None,
        }
    }

    /// Create a new activity log entry with error
    pub fn with_error(
        session_id: String,
        guest_id: String,
        activity_type: ActivityType,
        details: serde_json::Value,
        error: String,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            session_id,
            guest_id,
            activity_type,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
            details,
            status: ActivityStatus::Failed,
            error: Some(error),
        }
    }
}

/// Type of activity performed during a guest session
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ActivityType {
    /// Tool execution
    ToolCall {
        /// Name of the tool
        tool_name: String,
    },
    /// RPC request
    RpcRequest {
        /// RPC method name
        method: String,
    },
    /// Session event
    SessionEvent {
        /// Event name
        event: String,
    },
    /// Permission check
    PermissionCheck {
        /// Resource being checked
        resource: String,
    },
    /// Error occurred
    Error {
        /// Error type
        error_type: String,
    },
}

/// Status of an activity
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ActivityStatus {
    /// Activity completed successfully
    Success,
    /// Activity failed
    Failed,
    /// Activity is still pending
    Pending,
}

/// Query options for filtering activity logs
#[derive(Debug, Clone, Default)]
pub struct ActivityLogQuery {
    /// Maximum number of logs to return
    pub limit: Option<usize>,
    /// Offset for pagination
    pub offset: Option<usize>,
    /// Filter by activity type (serialized string)
    pub activity_type_filter: Option<String>,
    /// Filter by status
    pub status_filter: Option<ActivityStatus>,
    /// Start time (Unix milliseconds)
    pub start_time: Option<i64>,
    /// End time (Unix milliseconds)
    pub end_time: Option<i64>,
}

impl ActivityLogQuery {
    /// Create a new query with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Set limit
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set offset
    pub fn with_offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Set activity type filter
    pub fn with_activity_type(mut self, activity_type: String) -> Self {
        self.activity_type_filter = Some(activity_type);
        self
    }

    /// Set status filter
    pub fn with_status(mut self, status: ActivityStatus) -> Self {
        self.status_filter = Some(status);
        self
    }

    /// Set time range
    pub fn with_time_range(mut self, start: i64, end: i64) -> Self {
        self.start_time = Some(start);
        self.end_time = Some(end);
        self
    }

    /// Check if a log entry matches this query
    pub fn matches(&self, log: &GuestActivityLog) -> bool {
        // Check status filter
        if let Some(ref status) = self.status_filter {
            if &log.status != status {
                return false;
            }
        }

        // Check time range
        if let Some(start) = self.start_time {
            if log.timestamp < start {
                return false;
            }
        }
        if let Some(end) = self.end_time {
            if log.timestamp > end {
                return false;
            }
        }

        // Check activity type filter
        if let Some(ref type_filter) = self.activity_type_filter {
            let log_type = match &log.activity_type {
                ActivityType::ToolCall { .. } => "ToolCall",
                ActivityType::RpcRequest { .. } => "RpcRequest",
                ActivityType::SessionEvent { .. } => "SessionEvent",
                ActivityType::PermissionCheck { .. } => "PermissionCheck",
                ActivityType::Error { .. } => "Error",
            };
            if log_type != type_filter {
                return false;
            }
        }

        true
    }
}

/// Result of a query for activity logs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityLogQueryResult {
    /// Matching log entries
    pub logs: Vec<GuestActivityLog>,
    /// Total number of matching logs (before pagination)
    pub total: usize,
    /// Whether there are more logs available
    pub has_more: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_activity_log_creation() {
        let log = GuestActivityLog::new(
            "session-123".to_string(),
            "guest-456".to_string(),
            ActivityType::ToolCall {
                tool_name: "translate".to_string(),
            },
            serde_json::json!({"input": "hello"}),
            ActivityStatus::Success,
        );

        assert_eq!(log.session_id, "session-123");
        assert_eq!(log.guest_id, "guest-456");
        assert_eq!(log.status, ActivityStatus::Success);
        assert!(log.error.is_none());
    }

    #[test]
    fn test_activity_log_with_error() {
        let log = GuestActivityLog::with_error(
            "session-123".to_string(),
            "guest-456".to_string(),
            ActivityType::Error {
                error_type: "PermissionDenied".to_string(),
            },
            serde_json::json!({"resource": "tool:summarize"}),
            "Tool not allowed".to_string(),
        );

        assert_eq!(log.status, ActivityStatus::Failed);
        assert_eq!(log.error, Some("Tool not allowed".to_string()));
    }

    #[test]
    fn test_query_matches() {
        let log = GuestActivityLog::new(
            "session-123".to_string(),
            "guest-456".to_string(),
            ActivityType::ToolCall {
                tool_name: "translate".to_string(),
            },
            serde_json::json!({}),
            ActivityStatus::Success,
        );

        // Test status filter
        let query = ActivityLogQuery::new().with_status(ActivityStatus::Success);
        assert!(query.matches(&log));

        let query = ActivityLogQuery::new().with_status(ActivityStatus::Failed);
        assert!(!query.matches(&log));

        // Test activity type filter
        let query = ActivityLogQuery::new().with_activity_type("ToolCall".to_string());
        assert!(query.matches(&log));

        let query = ActivityLogQuery::new().with_activity_type("RpcRequest".to_string());
        assert!(!query.matches(&log));
    }
}
