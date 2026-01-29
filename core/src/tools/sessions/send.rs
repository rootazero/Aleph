//! sessions_send tool implementation.
//!
//! Sends a message to another session.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::types::SendStatus;

/// Parameters for sessions_send tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionsSendParams {
    /// Target session key (mutually exclusive with label)
    #[serde(default)]
    pub session_key: Option<String>,

    /// Target session label (mutually exclusive with session_key)
    #[serde(default)]
    pub label: Option<String>,

    /// Agent ID for label lookup (optional, defaults to current agent)
    #[serde(default)]
    pub agent_id: Option<String>,

    /// Message to send
    pub message: String,

    /// Timeout in seconds (0 = fire-and-forget, default: 30)
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
}

fn default_timeout() -> u32 {
    30
}

impl SessionsSendParams {
    /// Validate params
    pub fn validate(&self) -> Result<(), String> {
        if self.session_key.is_some() && self.label.is_some() {
            return Err("Provide either session_key or label, not both".into());
        }
        if self.session_key.is_none() && self.label.is_none() {
            return Err("session_key or label is required".into());
        }
        if self.message.trim().is_empty() {
            return Err("message cannot be empty".into());
        }
        Ok(())
    }

    /// Check if this is a fire-and-forget request
    pub fn is_fire_and_forget(&self) -> bool {
        self.timeout_seconds == 0
    }
}

/// Result of sessions_send tool
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionsSendResult {
    /// Send status
    pub status: SendStatus,
    /// Run ID for tracking
    pub run_id: Option<String>,
    /// Resolved session key
    pub session_key: Option<String>,
    /// Reply from target session (if waited)
    pub reply: Option<String>,
    /// Error message if failed
    pub error: Option<String>,
}

impl Default for SendStatus {
    fn default() -> Self {
        Self::Error
    }
}

impl SessionsSendResult {
    /// Create success result with reply
    pub fn ok(run_id: String, session_key: String, reply: Option<String>) -> Self {
        Self {
            status: SendStatus::Ok,
            run_id: Some(run_id),
            session_key: Some(session_key),
            reply,
            error: None,
        }
    }

    /// Create accepted result (fire-and-forget)
    pub fn accepted(run_id: String, session_key: String) -> Self {
        Self {
            status: SendStatus::Accepted,
            run_id: Some(run_id),
            session_key: Some(session_key),
            reply: None,
            error: None,
        }
    }

    /// Create forbidden result
    pub fn forbidden(error: impl Into<String>) -> Self {
        Self {
            status: SendStatus::Forbidden,
            error: Some(error.into()),
            ..Default::default()
        }
    }

    /// Create error result
    pub fn error(error: impl Into<String>) -> Self {
        Self {
            status: SendStatus::Error,
            error: Some(error.into()),
            ..Default::default()
        }
    }

    /// Create timeout result
    pub fn timeout(run_id: String, session_key: String) -> Self {
        Self {
            status: SendStatus::Timeout,
            run_id: Some(run_id),
            session_key: Some(session_key),
            reply: None,
            error: Some("Timeout waiting for reply".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_params_validation() {
        // Valid with session_key
        let params = SessionsSendParams {
            session_key: Some("agent:main:main".into()),
            label: None,
            agent_id: None,
            message: "Hello".into(),
            timeout_seconds: 30,
        };
        assert!(params.validate().is_ok());

        // Valid with label
        let params = SessionsSendParams {
            session_key: None,
            label: Some("my-session".into()),
            agent_id: None,
            message: "Hello".into(),
            timeout_seconds: 30,
        };
        assert!(params.validate().is_ok());

        // Invalid: both session_key and label
        let params = SessionsSendParams {
            session_key: Some("agent:main:main".into()),
            label: Some("my-session".into()),
            agent_id: None,
            message: "Hello".into(),
            timeout_seconds: 30,
        };
        assert!(params.validate().is_err());

        // Invalid: neither session_key nor label
        let params = SessionsSendParams {
            session_key: None,
            label: None,
            agent_id: None,
            message: "Hello".into(),
            timeout_seconds: 30,
        };
        assert!(params.validate().is_err());

        // Invalid: empty message
        let params = SessionsSendParams {
            session_key: Some("agent:main:main".into()),
            label: None,
            agent_id: None,
            message: "   ".into(),
            timeout_seconds: 30,
        };
        assert!(params.validate().is_err());
    }

    #[test]
    fn test_fire_and_forget() {
        let params = SessionsSendParams {
            session_key: Some("agent:main:main".into()),
            label: None,
            agent_id: None,
            message: "Hello".into(),
            timeout_seconds: 0,
        };
        assert!(params.is_fire_and_forget());
    }

    #[test]
    fn test_result_constructors() {
        let ok = SessionsSendResult::ok("run1".into(), "key1".into(), Some("reply".into()));
        assert_eq!(ok.status, SendStatus::Ok);
        assert!(ok.reply.is_some());

        let accepted = SessionsSendResult::accepted("run2".into(), "key2".into());
        assert_eq!(accepted.status, SendStatus::Accepted);

        let forbidden = SessionsSendResult::forbidden("Not allowed");
        assert_eq!(forbidden.status, SendStatus::Forbidden);
        assert!(forbidden.error.is_some());
    }
}
