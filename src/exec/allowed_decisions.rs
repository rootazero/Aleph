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

/// The full decision set offered for low-risk commands.
///
/// Doubles as the serde default for [`super::socket::ApprovalRequestPayload`],
/// so payloads serialized before this field existed deserialize to the
/// historical (unconstrained) behavior.
pub fn full_set() -> Vec<ApprovalDecisionType> {
    vec![
        ApprovalDecisionType::AllowOnce,
        ApprovalDecisionType::AllowAlways,
        ApprovalDecisionType::Deny,
    ]
}

/// The decisions permitted for a given assessed [`RiskLevel`].
///
/// - `Safe` / `Caution`: full set — remembering is fine.
/// - `Danger`: single-shot consent or denial only — never persist a
///   destructive command to the allowlist in one gesture.
/// - `Blocked`: deny only — defense in depth. A blocked command should be
///   hard-denied upstream and never reach an approval prompt; if one slips
///   through, the surface offers no path to run it.
pub fn decisions_for_risk(risk: RiskLevel) -> Vec<ApprovalDecisionType> {
    match risk {
        RiskLevel::Safe | RiskLevel::Caution => full_set(),
        RiskLevel::Danger => vec![ApprovalDecisionType::AllowOnce, ApprovalDecisionType::Deny],
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
pub fn assess_command_decisions(command: &str) -> Vec<ApprovalDecisionType> {
    decisions_for_risk(SecurityKernel::new().assess(command))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_set_has_three_decisions() {
        let set = full_set();
        assert_eq!(set.len(), 3);
        assert!(set.contains(&ApprovalDecisionType::AllowOnce));
        assert!(set.contains(&ApprovalDecisionType::AllowAlways));
        assert!(set.contains(&ApprovalDecisionType::Deny));
    }

    #[test]
    fn safe_and_caution_get_full_set() {
        assert_eq!(decisions_for_risk(RiskLevel::Safe), full_set());
        assert_eq!(decisions_for_risk(RiskLevel::Caution), full_set());
    }

    #[test]
    fn danger_drops_allow_always() {
        let set = decisions_for_risk(RiskLevel::Danger);
        assert!(set.contains(&ApprovalDecisionType::AllowOnce));
        assert!(set.contains(&ApprovalDecisionType::Deny));
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
    fn assess_safe_command_full_set() {
        // `ls -la` classifies Safe → remembering allowed.
        assert_eq!(assess_command_decisions("ls -la"), full_set());
    }

    #[test]
    fn assess_danger_command_no_remember() {
        // `rm -rf ./build` classifies Danger → no allow-always offered.
        let set = assess_command_decisions("rm -rf ./build");
        assert!(!set.contains(&ApprovalDecisionType::AllowAlways));
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
