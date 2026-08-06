//! Approval decision vocabulary shared by every approval surface.
//!
//! Historically this file also defined a Unix-socket UI protocol
//! (`SocketMessage` / `ApprovalRequestPayload`); that transport was never
//! wired into the server and has been removed. The decision enum is the
//! living wire vocabulary used by RPC handlers, channel callbacks and the
//! approval manager.

use serde::{Deserialize, Serialize};

use crate::sandbox::exec_approval::gate::ApprovalOutcome;

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

impl ApprovalDecisionType {
    /// Narrow the decision to a grant scope the system can actually honor.
    ///
    /// No persistent allowlist exists, so `AllowAlways` cannot outlive the
    /// process: it is a legacy wire value (old inline-keyboard callbacks,
    /// `/approve always` text replies, external RPC clients) and resolves to
    /// the session tier. An explicit human approval is never escalated or
    /// turned into a denial here — only the grant scope is narrowed. This is
    /// the ONE place that narrowing rule lives; resolvers
    /// ([`crate::exec::manager::ExecApprovalManager`]) and outcome mapping
    /// ([`Self::to_outcome`]) both go through it.
    #[must_use]
    pub const fn clamped(self) -> Self {
        match self {
            Self::AllowAlways => Self::AllowSession,
            other => other,
        }
    }

    /// The [`ApprovalOutcome`] this decision resolves to — the SINGLE decision
    /// → outcome mapping for every approval surface (channel bridge, operator
    /// requester, cluster node approval). `AllowAlways` narrows to the session
    /// grant via [`Self::clamped`], so the downgrade rule has one source.
    /// A missing decision (timeout / closed channel) is not representable
    /// here — callers map `None` to [`ApprovalOutcome::Timeout`] themselves.
    #[must_use]
    pub const fn to_outcome(self) -> ApprovalOutcome {
        match self.clamped() {
            Self::AllowOnce => ApprovalOutcome::Approved,
            Self::AllowSession => ApprovalOutcome::ApprovedForSession,
            // Unreachable post-clamp; kept for exhaustiveness.
            Self::AllowAlways => ApprovalOutcome::ApprovedForSession,
            Self::Deny => ApprovalOutcome::Denied,
        }
    }
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

    #[test]
    fn clamp_narrows_only_allow_always() {
        assert_eq!(
            ApprovalDecisionType::AllowAlways.clamped(),
            ApprovalDecisionType::AllowSession
        );
        // Other decisions pass through untouched; approvals are never escalated
        // or turned into denials here.
        for decision in [
            ApprovalDecisionType::AllowOnce,
            ApprovalDecisionType::AllowSession,
            ApprovalDecisionType::Deny,
        ] {
            assert_eq!(decision.clamped(), decision);
        }
    }

    /// The single decision → outcome mapping every approval surface shares.
    #[test]
    fn decision_maps_to_outcome() {
        assert_eq!(
            ApprovalDecisionType::AllowOnce.to_outcome(),
            ApprovalOutcome::Approved
        );
        assert_eq!(
            ApprovalDecisionType::AllowSession.to_outcome(),
            ApprovalOutcome::ApprovedForSession
        );
        // Legacy "allow always" degrades to the session grant — same narrowing
        // `clamped` applies at the decision layer.
        assert_eq!(
            ApprovalDecisionType::AllowAlways.to_outcome(),
            ApprovalOutcome::ApprovedForSession
        );
        assert_eq!(
            ApprovalDecisionType::Deny.to_outcome(),
            ApprovalOutcome::Denied
        );
    }
}
