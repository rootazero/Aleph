// Aleph/core/src/event/permission.rs
//! Permission-related event types for the unified permission system.
//!
//! These events enable async permission requests between the agent loop
//! and UI layer, following OpenCode's permission model.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

// Re-export PermissionAction from extension module
pub use crate::extension::PermissionAction;

// ============================================================================
// Permission Request Types
// ============================================================================

/// Reference to a tool call that triggered the permission request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRef {
    /// Message ID containing the tool call
    pub message_id: String,
    /// Tool call ID within the message
    pub call_id: String,
}

/// Permission request sent to UI for user confirmation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    /// Unique request ID
    pub id: String,
    /// Session ID
    pub session_id: String,
    /// Permission type (e.g., "edit", "bash", "read")
    pub permission: String,
    /// Patterns requiring authorization (e.g., file paths, command patterns)
    pub patterns: Vec<String>,
    /// Patterns that will be remembered if user selects "always"
    pub always_patterns: Vec<String>,
    /// Additional metadata (tool name, parameter preview, etc.)
    pub metadata: HashMap<String, Value>,
    /// Associated tool call reference (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<ToolCallRef>,
}

impl PermissionRequest {
    /// Create a new permission request
    pub fn new(
        id: impl Into<String>,
        session_id: impl Into<String>,
        permission: impl Into<String>,
        patterns: Vec<String>,
    ) -> Self {
        let patterns_clone = patterns.clone();
        Self {
            id: id.into(),
            session_id: session_id.into(),
            permission: permission.into(),
            patterns,
            always_patterns: patterns_clone,
            metadata: HashMap::new(),
            tool_call: None,
        }
    }

    /// Add metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Set tool call reference
    pub fn with_tool_call(mut self, tool_call: ToolCallRef) -> Self {
        self.tool_call = Some(tool_call);
        self
    }

    /// Set custom always patterns
    pub fn with_always_patterns(mut self, patterns: Vec<String>) -> Self {
        self.always_patterns = patterns;
        self
    }
}

// ============================================================================
// Permission Reply Types
// ============================================================================

/// User's reply to a permission request
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PermissionReply {
    /// Allow this specific request once
    Once,
    /// Allow and remember the rule (add to approved ruleset)
    Always,
    /// Reject the request
    Reject,
    /// Reject with feedback message (continues execution with guidance)
    Correct {
        /// User's feedback/correction message
        message: String,
    },
}

impl PermissionReply {
    /// Check if this reply allows the operation
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Once | Self::Always)
    }

    /// Check if this reply should persist the rule
    pub fn should_persist(&self) -> bool {
        matches!(self, Self::Always)
    }
}

// ============================================================================
// Permission Events
// ============================================================================

/// Permission-related events for the event bus
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PermissionEvent {
    /// Permission request sent to UI
    Asked(PermissionRequest),
    /// User replied to permission request
    Replied {
        session_id: String,
        request_id: String,
        reply: PermissionReply,
    },
}

impl PermissionEvent {
    /// Create an Asked event
    pub fn asked(request: PermissionRequest) -> Self {
        Self::Asked(request)
    }

    /// Create a Replied event
    pub fn replied(
        session_id: impl Into<String>,
        request_id: impl Into<String>,
        reply: PermissionReply,
    ) -> Self {
        Self::Replied {
            session_id: session_id.into(),
            request_id: request_id.into(),
            reply,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_request_builder() {
        let request = PermissionRequest::new("req-1", "session-1", "bash", vec!["git push".into()])
            .with_metadata("tool", serde_json::json!("bash"))
            .with_always_patterns(vec!["git *".into()]);

        assert_eq!(request.id, "req-1");
        assert_eq!(request.permission, "bash");
        assert_eq!(request.patterns, vec!["git push"]);
        assert_eq!(request.always_patterns, vec!["git *"]);
        assert!(request.metadata.contains_key("tool"));
    }

    #[test]
    fn test_permission_reply_serialization() {
        let reply = PermissionReply::Correct {
            message: "Use git pull instead".into(),
        };

        let json = serde_json::to_string(&reply).unwrap();
        assert!(json.contains("correct"));
        assert!(json.contains("Use git pull instead"));

        let parsed: PermissionReply = serde_json::from_str(&json).unwrap();
        assert!(!parsed.is_allowed());
    }

    #[test]
    fn test_permission_reply_checks() {
        assert!(PermissionReply::Once.is_allowed());
        assert!(PermissionReply::Always.is_allowed());
        assert!(!PermissionReply::Reject.is_allowed());

        assert!(!PermissionReply::Once.should_persist());
        assert!(PermissionReply::Always.should_persist());
    }

    #[test]
    fn test_permission_event_serialization() {
        let request =
            PermissionRequest::new("req-1", "session-1", "edit", vec!["src/main.rs".into()]);
        let event = PermissionEvent::asked(request);

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("Asked"));

        let replied = PermissionEvent::replied("session-1", "req-1", PermissionReply::Always);
        let json = serde_json::to_string(&replied).unwrap();
        assert!(json.contains("Replied"));
        assert!(json.contains("always"));
    }
}
