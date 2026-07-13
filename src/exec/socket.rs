//! Approval decision vocabulary shared by every approval surface.
//!
//! Historically this file also defined a Unix-socket UI protocol
//! (`SocketMessage` / `ApprovalRequestPayload`); that transport was never
//! wired into the server and has been removed. The decision enum is the
//! living wire vocabulary used by RPC handlers, channel callbacks and the
//! approval manager.

use serde::{Deserialize, Serialize};

/// Type of approval decision
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalDecisionType {
    /// Allow this execution only
    AllowOnce,
    /// Allow for the remainder of this session — remembered in the in-memory
    /// session approval store so the same command/tool is not re-prompted.
    /// The widest grant Aleph offers; mirrors codex `ApprovedForSession` and
    /// hermes' `session` consent tier.
    AllowSession,
    /// Legacy wire value for "allow forever". No persistent allowlist exists,
    /// so every resolver narrows this to [`Self::AllowSession`]
    /// (`ExecApprovalManager::clamp_decision`). Kept only so in-flight callback
    /// payloads and external clients still deserialize.
    AllowAlways,
    /// Deny execution
    Deny,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_approval_decision_types() {
        assert_eq!(
            serde_json::to_string(&ApprovalDecisionType::AllowOnce).unwrap(),
            r#""allow-once""#
        );
        assert_eq!(
            serde_json::to_string(&ApprovalDecisionType::AllowSession).unwrap(),
            r#""allow-session""#
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
