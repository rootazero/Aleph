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
    /// Allow for the remainder of this session — remembered in the grant
    /// store's session tier so the same command/tool is not re-prompted.
    /// Mirrors codex `ApprovedForSession` and hermes' `session` consent tier.
    AllowSession,
    /// Allow until revoked, across restarts — the persistent tier of
    /// [`GrantStore`](crate::sandbox::exec_approval::grants::GrantStore).
    ///
    /// Honored **only** when the approval record's `allowed_decisions` offered
    /// it ([`crate::exec::allowed_decisions`]); on every other card it narrows
    /// to [`Self::AllowSession`]. That narrowing is enforced server-side, at
    /// [`Self::clamped_for`], not by a renderer declining to draw the button:
    /// the wire value has always been accepted from external clients, so "no
    /// surface offers it" was never a control.
    AllowAlways,
    /// Deny execution
    Deny,
}

impl ApprovalDecisionType {
    /// Narrow the decision to a grant scope this particular request may honor.
    ///
    /// `allowed` is the decision set the card was *raised* with — one
    /// derivation, computed at the gate ([`crate::exec::allowed_decisions`]) and
    /// carried on the record — so the question "may this answer create a
    /// standing grant?" is answered where the gate knows the rule and the
    /// caller's tier, not where the button is drawn.
    ///
    /// Only the grant *scope* is narrowed: an explicit human approval is never
    /// escalated, and never turned into a denial. Narrowing twice is a no-op,
    /// so a defensive second call at a downstream surface is always safe.
    #[must_use]
    pub fn clamped_for(self, allowed: &[Self]) -> Self {
        // Walk DOWN the ladder one rung at a time rather than mapping straight
        // to the bottom: a card that offers `once` and `always` but not
        // `session` (the legacy backfill set) must narrow an unoffered tier to
        // the widest tier it DID offer, not to something it also never showed.
        let mut decision = self;
        loop {
            if allowed.contains(&decision) {
                return decision;
            }
            decision = match decision {
                Self::AllowAlways => Self::AllowSession,
                Self::AllowSession => Self::AllowOnce,
                // `AllowOnce` and `Deny` are the floor: a human's answer is
                // never escalated, and never turned into a refusal here.
                other => return other,
            };
        }
    }

    /// The [`ApprovalOutcome`] this decision resolves to, within the decision
    /// set the request offered — the SINGLE decision → outcome mapping for
    /// every approval surface (channel bridge, operator requester, cluster node
    /// approval).
    ///
    /// It takes `allowed` and there is no argument-free variant **on purpose**.
    /// The unsafe direction here is a surface that turns a widest-tier wire
    /// value into a persistent grant it was never authorized to create; making
    /// every call site name the set it offered turns that into a compile error
    /// rather than a rule someone has to remember (判据 §5.17: 编译错误强于
    /// 注册表 pin). Sites that never offer a standing grant pass
    /// [`crate::exec::allowed_decisions::session_max`] or
    /// [`crate::exec::allowed_decisions::once_only`].
    ///
    /// A missing decision (timeout / closed channel) is not representable
    /// here — callers map `None` to [`ApprovalOutcome::Timeout`] themselves.
    #[must_use]
    pub fn to_outcome_within(self, allowed: &[Self]) -> ApprovalOutcome {
        match self.clamped_for(allowed) {
            Self::AllowOnce => ApprovalOutcome::Approved,
            Self::AllowSession => ApprovalOutcome::ApprovedForSession,
            Self::AllowAlways => ApprovalOutcome::ApprovedAlways,
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
    fn clamp_narrows_to_what_the_card_offered() {
        let session_max = crate::exec::allowed_decisions::session_max();
        assert_eq!(
            ApprovalDecisionType::AllowAlways.clamped_for(&session_max),
            ApprovalDecisionType::AllowSession,
            "a card that never offered the persistent tier cannot produce one"
        );
        // Other decisions pass through untouched; approvals are never escalated
        // or turned into denials here.
        for decision in [
            ApprovalDecisionType::AllowOnce,
            ApprovalDecisionType::AllowSession,
            ApprovalDecisionType::Deny,
        ] {
            assert_eq!(decision.clamped_for(&session_max), decision);
        }
        // When the card DID offer it, the human's answer stands.
        let with_always = crate::exec::allowed_decisions::with_persistent();
        assert_eq!(
            ApprovalDecisionType::AllowAlways.clamped_for(&with_always),
            ApprovalDecisionType::AllowAlways
        );
        // Narrowing is idempotent, so a defensive second clamp downstream is
        // always safe.
        assert_eq!(
            ApprovalDecisionType::AllowAlways
                .clamped_for(&session_max)
                .clamped_for(&session_max),
            ApprovalDecisionType::AllowSession
        );
    }

    /// A card that only offers a one-shot allow (a cluster node approval, the
    /// escalation banner) narrows the session tier too — the same rule, one
    /// step further down.
    #[test]
    fn clamp_narrows_the_session_tier_when_it_was_not_offered() {
        let once = crate::exec::allowed_decisions::once_only();
        assert_eq!(
            ApprovalDecisionType::AllowSession.clamped_for(&once),
            ApprovalDecisionType::AllowOnce
        );
        assert_eq!(
            ApprovalDecisionType::AllowAlways.clamped_for(&once),
            ApprovalDecisionType::AllowOnce,
            "two narrowing steps, not a jump past the missing tier"
        );
    }

    /// The single decision → outcome mapping every approval surface shares.
    #[test]
    fn decision_maps_to_outcome() {
        let session_max = crate::exec::allowed_decisions::session_max();
        assert_eq!(
            ApprovalDecisionType::AllowOnce.to_outcome_within(&session_max),
            ApprovalOutcome::Approved
        );
        assert_eq!(
            ApprovalDecisionType::AllowSession.to_outcome_within(&session_max),
            ApprovalOutcome::ApprovedForSession
        );
        // "Allow always" on a card that did not offer it degrades to the
        // session grant — the historical behaviour, now conditional.
        assert_eq!(
            ApprovalDecisionType::AllowAlways.to_outcome_within(&session_max),
            ApprovalOutcome::ApprovedForSession
        );
        assert_eq!(
            ApprovalDecisionType::AllowAlways
                .to_outcome_within(&crate::exec::allowed_decisions::with_persistent()),
            ApprovalOutcome::ApprovedAlways
        );
        assert_eq!(
            ApprovalDecisionType::Deny.to_outcome_within(&session_max),
            ApprovalOutcome::Denied
        );
    }
}
