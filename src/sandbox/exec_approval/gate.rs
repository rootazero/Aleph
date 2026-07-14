use crate::sync_primitives::RwLock;
use std::sync::Arc;

use async_trait::async_trait;

#[async_trait]
pub trait ApprovalRequester: Send + Sync {
    async fn request_approval(&self, tool_name: &str, reason: &str) -> ApprovalOutcome;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOutcome {
    /// Approved for this single invocation only.
    Approved,
    /// Approved for the remainder of the session (user chose "allow always").
    /// Treated as approved everywhere; additionally recorded in the session
    /// approval memory so the same tool is not re-prompted this session.
    ApprovedForSession,
    Denied,
    Timeout,
}

impl ApprovalOutcome {
    #[must_use]
    pub const fn is_approved(&self) -> bool {
        matches!(self, Self::Approved | Self::ApprovedForSession)
    }

    /// True only for a session-scoped grant — the caller should remember it so
    /// subsequent calls to the same tool skip the prompt.
    #[must_use]
    pub const fn is_session_grant(&self) -> bool {
        matches!(self, Self::ApprovedForSession)
    }
}

pub struct ApprovalGate {
    /// Swappable so boot can construct the gate before the channel registry
    /// exists, then wire the real requester via `set_requester` once channels
    /// are up. An empty slot denies — never a silent auto-approve.
    requester: RwLock<Option<Arc<dyn ApprovalRequester>>>,
}

impl ApprovalGate {
    #[must_use]
    pub fn new(requester: Option<Arc<dyn ApprovalRequester>>) -> Self {
        Self {
            requester: RwLock::new(requester),
        }
    }

    /// Install (or replace) the approval requester after construction.
    ///
    /// Boot constructs the gate before the channel registry exists; once
    /// channels are up this wires the real `ChannelApprovalBridgeAdapter` so
    /// elevated-capability sandbox escalations can actually reach the user
    /// instead of being auto-denied.
    pub fn set_requester(&self, requester: Arc<dyn ApprovalRequester>) {
        *self.requester.write().unwrap_or_else(|e| e.into_inner()) = Some(requester);
    }

    pub async fn request_approval_for_tool(
        &self,
        tool_name: &str,
        reason: &str,
    ) -> ApprovalOutcome {
        // Clone the Arc out of the lock and drop the guard before awaiting —
        // a std `RwLock` guard is not `Send` and must not be held across await.
        let requester = self
            .requester
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match requester {
            Some(requester) => requester.request_approval(tool_name, reason).await,
            None => {
                tracing::warn!("No approval requester configured, defaulting to denied");
                ApprovalOutcome::Denied
            }
        }
    }
}

/// The gate is itself an [`ApprovalRequester`], delegating to its own
/// late-bound requester via [`request_approval_for_tool`].
///
/// This lets one shared `ApprovalGate` (whose channel requester is wired after
/// channels are up) serve both the sandbox escalation path and the failover
/// route escalation (borrow-cloud) gate, without exposing the inner requester.
///
/// [`request_approval_for_tool`]: ApprovalGate::request_approval_for_tool
#[async_trait]
impl ApprovalRequester for ApprovalGate {
    async fn request_approval(&self, tool_name: &str, reason: &str) -> ApprovalOutcome {
        self.request_approval_for_tool(tool_name, reason).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_outcome_is_approved() {
        assert!(ApprovalOutcome::Approved.is_approved());
        assert!(!ApprovalOutcome::Denied.is_approved());
        assert!(!ApprovalOutcome::Timeout.is_approved());
    }

    /// Regression: the gate denies when no requester is wired (the CRITICAL
    /// boot bug), and routes to the requester once `set_requester` installs
    /// one — the post-construction wiring boot now performs.
    #[tokio::test]
    async fn set_requester_makes_escalation_reach_the_requester() {
        struct AlwaysApprove;
        #[async_trait::async_trait]
        impl ApprovalRequester for AlwaysApprove {
            async fn request_approval(&self, _tool: &str, _reason: &str) -> ApprovalOutcome {
                ApprovalOutcome::Approved
            }
        }

        let gate = ApprovalGate::new(None);
        // No requester wired → denied (never a silent auto-approve).
        assert_eq!(
            gate.request_approval_for_tool("code_exec", "allow_network")
                .await,
            ApprovalOutcome::Denied
        );
        // Once the requester is installed, escalations reach it.
        gate.set_requester(Arc::new(AlwaysApprove));
        assert_eq!(
            gate.request_approval_for_tool("code_exec", "allow_network")
                .await,
            ApprovalOutcome::Approved
        );
    }
}
