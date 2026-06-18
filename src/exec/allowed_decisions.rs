//! Single source of truth for which approval decisions a command may offer.
//!
//! Three independent reference agents converge on the same design: an approval
//! request should declare the decision set it permits, and a high-risk command
//! must NOT offer a one-click "remember forever" option:
//! - codex carries `available_decisions: Vec<ReviewDecision>` on every request;
//! - openclaw carries `allowedDecisions` and drops `allow-always` under
//!   `ask = "always"`;
//! - hermes hides the `[a]lways` choice whenever a security finding is present,
//!   permitting only session-scoped consent.
//!
//! Aleph already classifies command risk via [`SecurityKernel`], but that
//! classifier was never wired into the approval surface — every renderer
//! hardcoded the full `Once / Always / Deny` button set, so a destructive
//! command (`rm`, `sudo`, env-injection, …) could be permanently allowlisted in
//! a single click. This module is that wiring point: it derives the permitted
//! decision set from the assessed [`RiskLevel`] and is the one place every
//! renderer (inline keyboard, chat reply-hints, socket payload) consults.

use super::kernel::SecurityKernel;
use super::risk::RiskLevel;
use super::socket::ApprovalDecisionType;

/// The legacy three-decision set (`Once` / `Always` / `Deny`).
///
/// Doubles as the serde default for
/// [`crate::exec::approval::types::CommandApprovalRequest::allowed_decisions`],
/// so payloads serialized before the `allowed_decisions` field existed
/// deserialize to the historical (unconstrained) behavior. Kept byte-stable on
/// purpose: it backfills *old* payloads, so it must NOT gain the newer
/// `AllowSession` tier — live requests get their set from [`decisions_for_risk`]
/// instead.
#[must_use]
pub fn full_set() -> Vec<ApprovalDecisionType> {
    vec![
        ApprovalDecisionType::AllowOnce,
        ApprovalDecisionType::AllowAlways,
        ApprovalDecisionType::Deny,
    ]
}

/// The decisions permitted for a given assessed [`RiskLevel`].
///
/// Three independent reference agents converge on a *session* consent tier that
/// sits between single-shot and permanent: codex's `ApprovedForSession`, hermes'
/// `session` choice. Aleph already owns the machinery
/// ([`crate::sandbox::exec_approval::session_memory`] +
/// `ApprovalOutcome::ApprovedForSession`); this is where that tier is offered.
///
/// - `Safe` / `Caution`: every tier — once, session, persist, or deny.
/// - `Danger`: once **or session** or deny — a destructive command may be
///   trusted for the rest of the session (so a build loop isn't re-prompted)
///   but is still never written to the on-disk allowlist in one gesture.
/// - `Blocked`: deny only — defense in depth. A blocked command should be
///   hard-denied upstream and never reach an approval prompt; if one slips
///   through, the surface offers no path to run it.
#[must_use]
pub fn decisions_for_risk(risk: RiskLevel) -> Vec<ApprovalDecisionType> {
    match risk {
        RiskLevel::Safe | RiskLevel::Caution => vec![
            ApprovalDecisionType::AllowOnce,
            ApprovalDecisionType::AllowSession,
            ApprovalDecisionType::AllowAlways,
            ApprovalDecisionType::Deny,
        ],
        RiskLevel::Danger => vec![
            ApprovalDecisionType::AllowOnce,
            ApprovalDecisionType::AllowSession,
            ApprovalDecisionType::Deny,
        ],
        RiskLevel::Blocked => vec![ApprovalDecisionType::Deny],
    }
}

/// Assess a raw command and derive its permitted decision set.
///
/// This is the seam that connects the otherwise-dormant [`SecurityKernel`]
/// risk classifier into the approval surface. Pure and stateless — the regex
/// assessment runs on a single command string, so recomputing it at each
/// render site is negligible and keeps every renderer in agreement without
/// threading extra state through the record types.
#[must_use]
pub fn assess_command_decisions(command: &str) -> Vec<ApprovalDecisionType> {
    let kernel = SecurityKernel::new();
    // Assess the whole command (preserves operator-spanning patterns such as
    // `curl ... | sh`) AND each chain/pipe segment. Most danger patterns are
    // `^`-anchored, so a destructive command can otherwise be hidden behind a
    // safe one — `ls && rm -rf .` would assess as Safe on the raw string. Each
    // segment is assessed independently and the max (most restrictive) risk is
    // taken, so this can only tighten, never loosen, classification.
    let mut risk = kernel.assess(command);
    for segment in risk_segments(command) {
        risk = risk.max(kernel.assess(segment));
    }
    decisions_for_risk(risk)
}

/// Split a command line into chain/pipeline segments for risk assessment.
///
/// Conservative and quote-naive: over-splitting only ever raises the assessed
/// risk (segments are assessed independently and the max is taken), so a
/// quoted operator can at worst yield an extra approval prompt — never a missed
/// danger classification.
fn risk_segments(command: &str) -> Vec<&str> {
    command
        .split(|c| c == ';' || c == '\n' || c == '|' || c == '&')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
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

    #[test]
    fn safe_and_caution_offer_session_tier() {
        let expected = vec![
            ApprovalDecisionType::AllowOnce,
            ApprovalDecisionType::AllowSession,
            ApprovalDecisionType::AllowAlways,
            ApprovalDecisionType::Deny,
        ];
        assert_eq!(decisions_for_risk(RiskLevel::Safe), expected);
        assert_eq!(decisions_for_risk(RiskLevel::Caution), expected);
    }

    #[test]
    fn danger_offers_session_but_not_always() {
        let set = decisions_for_risk(RiskLevel::Danger);
        assert!(set.contains(&ApprovalDecisionType::AllowOnce));
        assert!(set.contains(&ApprovalDecisionType::Deny));
        assert!(
            set.contains(&ApprovalDecisionType::AllowSession),
            "a destructive command may still be trusted for the rest of the session"
        );
        assert!(
            !set.contains(&ApprovalDecisionType::AllowAlways),
            "a destructive command must not be permanently allowlistable in one click"
        );
    }

    #[test]
    fn blocked_offers_only_deny() {
        let set = decisions_for_risk(RiskLevel::Blocked);
        assert_eq!(set, vec![ApprovalDecisionType::Deny]);
    }

    #[test]
    fn assess_safe_command_offers_session_tier() {
        // `ls -la` classifies Safe → every tier including session.
        assert_eq!(
            assess_command_decisions("ls -la"),
            decisions_for_risk(RiskLevel::Safe)
        );
    }

    #[test]
    fn assess_danger_command_session_not_remember() {
        // `rm -rf ./build` classifies Danger → session ok, allow-always not.
        let set = assess_command_decisions("rm -rf ./build");
        assert!(!set.contains(&ApprovalDecisionType::AllowAlways));
        assert!(set.contains(&ApprovalDecisionType::AllowSession));
        assert!(set.contains(&ApprovalDecisionType::AllowOnce));
        assert!(set.contains(&ApprovalDecisionType::Deny));
    }

    #[test]
    fn assess_blocked_command_only_deny() {
        // `rm -rf /` classifies Blocked → deny is the only offered path.
        assert_eq!(
            assess_command_decisions("rm -rf /"),
            vec![ApprovalDecisionType::Deny]
        );
    }
}
