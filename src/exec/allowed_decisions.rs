//! The decision set an approval request may offer.
//!
//! What narrows the set is [`crate::exec::manager::ExecApprovalManager::clamp_decision`]
//! (it refuses `AllowAlways` globally) plus the renderers, which never emit an
//! "always" button. This module carries only the legacy set that keeps
//! pre-`allowed_decisions` payloads deserializing.

use super::socket::ApprovalDecisionType;

/// The legacy three-decision set (`Once` / `Always` / `Deny`).
///
/// Sole purpose: the serde default for
/// [`crate::exec::approval::types::CommandApprovalRequest::allowed_decisions`],
/// so payloads serialized before that field existed deserialize to the
/// historical (unconstrained) behavior. Byte-stable on purpose — it backfills
/// *old* payloads, so it must NOT gain the newer `AllowSession` tier. Live
/// requests build their own set at the render site.
#[must_use]
pub fn full_set() -> Vec<ApprovalDecisionType> {
    vec![
        ApprovalDecisionType::AllowOnce,
        ApprovalDecisionType::AllowAlways,
        ApprovalDecisionType::Deny,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_set_stays_three_for_serde_backfill() {
        // `full_set` is the on-the-wire backfill default for pre-`allowed_decisions`
        // payloads and must remain the historical three tiers — adding the newer
        // `AllowSession` here would silently rewrite old payloads.
        let set = full_set();
        assert_eq!(set.len(), 3);
        assert!(set.contains(&ApprovalDecisionType::AllowOnce));
        assert!(set.contains(&ApprovalDecisionType::AllowAlways));
        assert!(set.contains(&ApprovalDecisionType::Deny));
        assert!(!set.contains(&ApprovalDecisionType::AllowSession));
    }
}
