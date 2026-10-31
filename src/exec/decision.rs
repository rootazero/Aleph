//! Approval request shape for command execution.

use super::allowed_decisions;
use super::analysis::CommandAnalysis;
use crate::exec::socket::ApprovalDecisionType;

/// Request for user approval
#[derive(Debug, Clone)]
pub struct ExecApprovalRequest {
    /// Unique request ID
    pub id: String,
    /// Full command string
    pub command: String,
    /// Working directory
    pub cwd: Option<String>,
    /// Command analysis result
    pub analysis: CommandAnalysis,
    /// Agent ID
    pub agent_id: String,
    /// Session key
    pub session_key: String,
    /// Why approval is being requested (escalation / confirmation context),
    /// surfaced to the resolving user. `None` when the command itself is the
    /// full context (plain exec approval).
    pub reason: Option<String>,
    /// Raw channel user id of the human whose message triggered this approval —
    /// the "originator". Set only for channel-originated tool runs; `None` for
    /// Panel / cron / internal / cluster approvals. Carried onto
    /// [`crate::exec::manager::ExecApprovalRecord`] so the channel button
    /// callback can refuse a resolution from anyone but the originator — closing
    /// the group-chat bypass where any paired member could approve another
    /// member's action. See `ManagerCallbackSink::handle_callback`.
    pub originator_user_id: Option<String>,
    /// Session-grant identity of the action being approved
    /// ([`crate::sandbox::exec_approval::ApprovalAction::grant_key`]). `None`
    /// for approvals with no action identity (cluster node approvals, bare
    /// route escalations) — those never participate in the session-grant
    /// cascade.
    pub grant_key: Option<String>,
    /// The decision tiers this card may offer, from
    /// [`ApprovalAction::allowed_decisions`] — derived once at the gate and
    /// enforced at the resolver, so a client that sends a wider wire value than
    /// the card offered is narrowed rather than obeyed.
    ///
    /// [`ApprovalAction::allowed_decisions`]: crate::sandbox::exec_approval::ApprovalAction::allowed_decisions
    pub allowed_decisions: Vec<crate::exec::socket::ApprovalDecisionType>,
}

/// Backward compatibility type alias
pub type ApprovalRequest = ExecApprovalRequest;

impl ExecApprovalRequest {
    /// The decision tiers this card may actually offer.
    ///
    /// `allowed_decisions` is `pub` and reaches this type unvalidated, so it
    /// can be empty — and an empty set renders a card with **no** buttons, not
    /// even "Deny", so the approval could only end in a timeout. The fallback
    /// is [`once_only`], the narrowest answerable set: the card stays
    /// answerable, and the fallback can only narrow a grant, never widen one.
    ///
    /// [`once_only`]: crate::exec::allowed_decisions::once_only
    #[must_use]
    pub fn offered_decisions(&self) -> Vec<ApprovalDecisionType> {
        if self.allowed_decisions.is_empty() {
            return allowed_decisions::once_only();
        }
        self.allowed_decisions.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_with(decisions: Vec<ApprovalDecisionType>) -> ExecApprovalRequest {
        ExecApprovalRequest {
            id: "req-1".to_string(),
            command: "echo hi".to_string(),
            cwd: None,
            analysis: CommandAnalysis::not_a_command(),
            agent_id: "agent-1".to_string(),
            session_key: "session-1".to_string(),
            reason: None,
            originator_user_id: None,
            grant_key: None,
            allowed_decisions: decisions,
        }
    }

    /// An empty set must not render a card with zero buttons — a card you can
    /// neither take nor refuse is not a card.
    #[test]
    fn an_empty_decision_set_falls_back_to_the_narrowest_answerable_one() {
        let offered = request_with(Vec::new()).offered_decisions();

        assert_eq!(offered, allowed_decisions::once_only());
        assert!(offered.contains(&ApprovalDecisionType::AllowOnce));
        assert!(offered.contains(&ApprovalDecisionType::Deny));
    }

    /// The fallback applies to the empty set only: a set derived at the gate is
    /// passed through unchanged.
    #[test]
    fn a_non_empty_decision_set_is_passed_through_unchanged() {
        let offered = request_with(allowed_decisions::session_max()).offered_decisions();

        assert_eq!(offered, allowed_decisions::session_max());
    }
}
