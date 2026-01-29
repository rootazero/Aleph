//! Socket protocol for approval communication.
//!
//! Defines the JSON message format for UI-Core communication.

use serde::{Deserialize, Serialize};

/// Message sent over the approval socket
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SocketMessage {
    /// Request for approval (Core -> UI)
    Request {
        /// Authentication token
        token: String,
        /// Request ID
        id: String,
        /// Request payload
        request: ApprovalRequestPayload,
    },

    /// Decision response (UI -> Core)
    Decision {
        /// Request ID being answered
        id: String,
        /// The decision
        decision: ApprovalDecisionType,
    },

    /// Error message
    Error {
        /// Error message
        message: String,
    },
}

/// Payload for an approval request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequestPayload {
    /// Full command string
    pub command: String,

    /// Working directory
    pub cwd: Option<String>,

    /// Agent ID
    pub agent_id: String,

    /// Session key
    pub session_key: String,

    /// Primary executable name
    pub executable: String,

    /// Resolved path (if found)
    pub resolved_path: Option<String>,

    /// All command segments
    pub segments: Vec<SegmentInfo>,
}

/// Information about a command segment for display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentInfo {
    /// Raw command text
    pub raw: String,

    /// Executable name
    pub executable: String,

    /// Resolved path
    pub resolved_path: Option<String>,

    /// Arguments (excluding executable)
    pub args: Vec<String>,
}

/// Type of approval decision
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalDecisionType {
    /// Allow this execution only
    AllowOnce,
    /// Allow and add to allowlist
    AllowAlways,
    /// Deny execution
    Deny,
}

impl ApprovalRequestPayload {
    /// Create from approval request
    pub fn from_request(request: &super::decision::ApprovalRequest) -> Self {
        let segments: Vec<SegmentInfo> = request
            .analysis
            .segments
            .iter()
            .map(|s| SegmentInfo {
                raw: s.raw.clone(),
                executable: s
                    .resolution
                    .as_ref()
                    .map(|r| r.executable_name.clone())
                    .unwrap_or_else(|| s.argv.first().cloned().unwrap_or_default()),
                resolved_path: s.resolution.as_ref().and_then(|r| {
                    r.resolved_path.as_ref().map(|p| p.to_string_lossy().into())
                }),
                args: s.argv.iter().skip(1).cloned().collect(),
            })
            .collect();

        let primary = segments.first();

        Self {
            command: request.command.clone(),
            cwd: request.cwd.clone(),
            agent_id: request.agent_id.clone(),
            session_key: request.session_key.clone(),
            executable: primary.map(|s| s.executable.clone()).unwrap_or_default(),
            resolved_path: primary.and_then(|s| s.resolved_path.clone()),
            segments,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_socket_message_request_serialize() {
        let msg = SocketMessage::Request {
            token: "secret".into(),
            id: "req-123".into(),
            request: ApprovalRequestPayload {
                command: "npm install".into(),
                cwd: Some("/project".into()),
                agent_id: "main".into(),
                session_key: "agent:main:main".into(),
                executable: "npm".into(),
                resolved_path: Some("/usr/bin/npm".into()),
                segments: vec![SegmentInfo {
                    raw: "npm install".into(),
                    executable: "npm".into(),
                    resolved_path: Some("/usr/bin/npm".into()),
                    args: vec!["install".into()],
                }],
            },
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"request""#));
        assert!(json.contains(r#""token":"secret""#));
    }

    #[test]
    fn test_socket_message_decision_serialize() {
        let msg = SocketMessage::Decision {
            id: "req-123".into(),
            decision: ApprovalDecisionType::AllowOnce,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"decision""#));
        assert!(json.contains(r#""decision":"allow-once""#));
    }

    #[test]
    fn test_socket_message_deserialize() {
        let json = r#"{"type":"decision","id":"req-123","decision":"allow-always"}"#;
        let msg: SocketMessage = serde_json::from_str(json).unwrap();

        assert!(matches!(
            msg,
            SocketMessage::Decision {
                decision: ApprovalDecisionType::AllowAlways,
                ..
            }
        ));
    }

    #[test]
    fn test_approval_decision_types() {
        assert_eq!(
            serde_json::to_string(&ApprovalDecisionType::AllowOnce).unwrap(),
            r#""allow-once""#
        );
        assert_eq!(
            serde_json::to_string(&ApprovalDecisionType::AllowAlways).unwrap(),
            r#""allow-always""#
        );
        assert_eq!(
            serde_json::to_string(&ApprovalDecisionType::Deny).unwrap(),
            r#""deny""#
        );
    }
}
