//! Approval decision vocabulary shared by every approval surface.
//!
//! Historically this file also defined a Unix-socket UI protocol
//! (`SocketMessage` / `ApprovalRequestPayload`); that transport was never
//! wired into the server and has been removed. The decision enum is the
//! living wire vocabulary used by RPC handlers, channel callbacks and the
//! approval manager.

pub use aleph_slot_api::approval::ApprovalDecisionType;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;

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
