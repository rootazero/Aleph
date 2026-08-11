//! The decision set an approval request may offer — **one derivation**, read
//! by every face and enforced at the resolver.
//!
//! A card's buttons and the server's willingness to honour a decision are the
//! same fact. When they were two facts, they were two facts that could not be
//! compared: the Panel hardcoded three buttons, Telegram read
//! `allowed_decisions`, and `AllowAlways` was narrowed unconditionally in a
//! third place — so "no surface offers it" was doing the work of a control,
//! while the wire value stayed accepted from any RPC client.
//!
//! Now the set is derived HERE, once, at the gate that knows *why* the call
//! stopped and *who* is being asked; it is carried on
//! [`ApprovalAction`](crate::sandbox::exec_approval::ApprovalAction) → the
//! pending record → every renderer; and it is what
//! [`ApprovalDecisionType::clamped_for`] enforces when the answer comes back.
//! A face that draws fewer buttons is therefore only ever *narrower* than the
//! rule, never wider.
//!
//! # Who may create a persistent grant
//!
//! [`for_confirm_gate`] adds [`ApprovalDecisionType::AllowAlways`] only when
//! both hold:
//!
//! 1. **The turn is operator-tier** (`TurnContext::caller_is_operator`, the
//!    same predicate the operator tool gate uses — one derivation, not a second
//!    spelling). A persistent grant is install-wide, exactly like the
//!    `[policies.tool_permissions]` `allow` entry it is the per-call sibling of,
//!    so a member creating one would silently authorize everyone else's
//!    identical call. On an operator *escalation* card — raised precisely
//!    because the requester is not operator-tier — this is false, so answering
//!    "always" cannot permanently strip an operator gate a member has to pass.
//! 2. **The gate is not the tool's own declared floor** (`tool_declared`). That
//!    rule's card says, in the sentence the same card renders, that it "asks
//!    under every execution tier — including `full` — and an `allow` entry does
//!    not switch it off". Offering a button that would switch it off makes that
//!    sentence false at the moment it is read, which is the most expensive
//!    place in this repo to be wrong (判据 §0: 一句关于"什么被闸住"的话，往往有
//!    三份拷贝，其中一份是发给模型的).

use super::socket::ApprovalDecisionType;

/// The legacy three-decision set (`Once` / `Always` / `Deny`).
///
/// Sole purpose: the serde default for
/// [`crate::exec::approval::types::CommandApprovalRequest::allowed_decisions`],
/// so payloads serialized before that field existed deserialize to the
/// historical (unconstrained) behavior. Byte-stable on purpose — it backfills
/// *old* payloads, so it must NOT gain the newer `AllowSession` tier. Live
/// requests build their own set at the render site.
///
/// It is **not** the serde default for a pending record's set
/// ([`session_max`] is): a missing field must never widen what a decision may
/// become, and this one contains `AllowAlways`.
#[must_use]
pub fn full_set() -> Vec<ApprovalDecisionType> {
    vec![
        ApprovalDecisionType::AllowOnce,
        ApprovalDecisionType::AllowAlways,
        ApprovalDecisionType::Deny,
    ]
}

/// Allow-once and deny — for approvals that cannot carry a standing grant at
/// all (a cluster node's reverse-RPC approval, a route escalation).
#[must_use]
pub fn once_only() -> Vec<ApprovalDecisionType> {
    vec![ApprovalDecisionType::AllowOnce, ApprovalDecisionType::Deny]
}

/// Everything up to and including the session tier — the default ceiling, and
/// the serde backfill for a record persisted before the field existed.
#[must_use]
pub fn session_max() -> Vec<ApprovalDecisionType> {
    vec![
        ApprovalDecisionType::AllowOnce,
        ApprovalDecisionType::AllowSession,
        ApprovalDecisionType::Deny,
    ]
}

/// [`session_max`] plus the persistent tier.
#[must_use]
pub fn with_persistent() -> Vec<ApprovalDecisionType> {
    vec![
        ApprovalDecisionType::AllowOnce,
        ApprovalDecisionType::AllowSession,
        ApprovalDecisionType::AllowAlways,
        ApprovalDecisionType::Deny,
    ]
}

/// The set a **tool confirmation** card may offer.
///
/// `rule_id` is [`GateRule::id`](crate::tools::scoped::gate_chain) — the stable
/// token naming which rule stopped the call — and `caller_is_operator` is the
/// requesting turn's tier. See the module docs for why those two, and only
/// those two, decide it.
#[must_use]
pub fn for_confirm_gate(rule_id: &str, caller_is_operator: bool) -> Vec<ApprovalDecisionType> {
    if caller_is_operator && rule_id != DECLARED_FLOOR_RULE {
        with_persistent()
    } else {
        session_max()
    }
}

/// The rule id whose card must never offer a persistent grant. Named here
/// rather than matched inline so the coupling to `GateRule::ToolDeclared` is
/// greppable from both ends.
pub const DECLARED_FLOOR_RULE: &str = "tool_declared";

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

    /// The two conditions, each on its own: this is the production narrowing
    /// scenario `allowed_decisions` exists for.
    #[test]
    fn only_an_operator_turn_outside_the_declared_floor_may_persist() {
        assert!(for_confirm_gate("tier_raised", true).contains(&ApprovalDecisionType::AllowAlways));
        assert!(
            !for_confirm_gate("tier_raised", false).contains(&ApprovalDecisionType::AllowAlways),
            "a member's card must not offer an install-wide grant"
        );
        assert!(
            !for_confirm_gate(DECLARED_FLOOR_RULE, true)
                .contains(&ApprovalDecisionType::AllowAlways),
            "the declared floor's own card says nothing can switch it off"
        );
        assert!(!for_confirm_gate(DECLARED_FLOOR_RULE, false)
            .contains(&ApprovalDecisionType::AllowAlways));
    }

    /// Every set a live card can be raised with keeps the two decisions that
    /// are never optional — a card you can neither take nor refuse is not a
    /// card.
    #[test]
    fn every_live_set_can_be_answered_both_ways() {
        for set in [
            once_only(),
            session_max(),
            with_persistent(),
            for_confirm_gate("policy_ask", true),
            for_confirm_gate("policy_ask", false),
        ] {
            assert!(set.contains(&ApprovalDecisionType::AllowOnce));
            assert!(set.contains(&ApprovalDecisionType::Deny));
        }
    }

    /// The token has to keep meaning what the card says. `GateRule::id` reads
    /// this same constant for its `ToolDeclared` arm, so the two cannot drift;
    /// this pins the *value*, which is what already-written ledger rows, logs
    /// and tests key on.
    #[test]
    fn the_declared_floor_token_is_stable() {
        assert_eq!(DECLARED_FLOOR_RULE, "tool_declared");
    }
}
