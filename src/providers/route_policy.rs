//! R7-safe local/cloud route policy selector.
//!
//! A self-contained, prompt-blind module that turns two HARD signals — the
//! operator's [`RouteMode`] (an explicit user choice) and a candidate's
//! [`EndpointTier`] (a `base_url`-derived connectivity fact) — into a
//! [`CandidateAction`]. It never sees messages, tools, or the prompt; it
//! cannot classify task intent. Choosing local vs cloud is *infrastructure*
//! (R7 赋能层), and the resulting candidate SET is still handed to the existing
//! failover engine which owns all retry / breaker / model-walk logic (R10 dumb
//! loop preserved — the harness never learns the route mode).
//!
//! In [`RouteMode::Auto`] every candidate is [`Allow`](CandidateAction::Allow)
//! in original order, so the policy is a no-op (byte-identical to pre-route
//! failover).

use crate::config::types::RouteMode;
use crate::providers::model_catalog::EndpointKind;

/// Runtime endpoint tier carried alongside each failover candidate.
///
/// [`Unknown`](EndpointTier::Unknown) is the live-primary slot whose `base_url`
/// is not resolvable at ordering time — it is the operator's configured
/// default and is always allowed in every mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointTier {
    /// On-machine / local-network endpoint.
    Local,
    /// Public-API endpoint.
    Cloud,
    /// Tier not resolvable (the live primary slot). Always allowed.
    Unknown,
}

impl From<EndpointKind> for EndpointTier {
    fn from(k: EndpointKind) -> Self {
        match k {
            EndpointKind::Local => EndpointTier::Local,
            EndpointKind::Cloud => EndpointTier::Cloud,
        }
    }
}

/// What the route policy decides for one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateAction {
    /// Keep the candidate in the chain unconditionally.
    Allow,
    /// Keep the candidate, but it crosses the preferred tier. When
    /// `requires_approval` is `true` the failover walk must obtain user
    /// approval before dialing it (borrow-cloud); when `false` it is an
    /// ungated safe degrade (cloud → local).
    CrossTier { requires_approval: bool },
    /// Drop the candidate from the chain entirely.
    Skip,
}

/// Classify one candidate from the two hard signals.
///
/// Pure total function over `(mode, tier, allow_cloud_escalation)`:
///
/// | mode          | tier=Local            | tier=Cloud                              | tier=Unknown |
/// |---------------|-----------------------|-----------------------------------------|--------------|
/// | `Auto`        | Allow                 | Allow                                   | Allow        |
/// | `AlwaysLocal` | Allow                 | escalate? CrossTier{approval} : Skip    | Allow        |
/// | `AlwaysCloud` | CrossTier{no-approval}| Allow                                   | Allow        |
///
/// The `Unknown` (live-primary) slot is always allowed: route mode shapes the
/// *fallbacks* around the operator's configured default, never overrides it.
pub fn classify_candidate(
    mode: RouteMode,
    tier: EndpointTier,
    allow_cloud_escalation: bool,
) -> CandidateAction {
    match (mode, tier) {
        // Auto never shapes the chain.
        (RouteMode::Auto, _) => CandidateAction::Allow,

        // The configured-default primary is always allowed.
        (_, EndpointTier::Unknown) => CandidateAction::Allow,

        // AlwaysLocal: keep local; cloud is escalation-gated or dropped.
        (RouteMode::AlwaysLocal, EndpointTier::Local) => CandidateAction::Allow,
        (RouteMode::AlwaysLocal, EndpointTier::Cloud) => {
            if allow_cloud_escalation {
                CandidateAction::CrossTier {
                    requires_approval: true,
                }
            } else {
                CandidateAction::Skip
            }
        }

        // AlwaysCloud: keep cloud; local is an ungated last-resort degrade.
        (RouteMode::AlwaysCloud, EndpointTier::Cloud) => CandidateAction::Allow,
        (RouteMode::AlwaysCloud, EndpointTier::Local) => CandidateAction::CrossTier {
            requires_approval: false,
        },
    }
}

/// Order and gate a candidate list under `mode`.
///
/// Partitions into `Allow` candidates (preferred tier, *original relative
/// order preserved*) followed by `CrossTier` crossings appended LAST, dropping
/// every `Skip`. Each retained candidate is paired with the
/// [`CandidateAction`] the failover walk must enforce. Generic over `T` so it
/// is unit-testable without real providers.
///
/// `tier_of` extracts a candidate's [`EndpointTier`]; the policy is computed
/// once per candidate.
pub fn order_candidates<T, F>(
    candidates: Vec<T>,
    mode: RouteMode,
    allow_cloud_escalation: bool,
    tier_of: F,
) -> Vec<(T, CandidateAction)>
where
    F: Fn(&T) -> EndpointTier,
{
    let mut same_tier: Vec<(T, CandidateAction)> = Vec::new();
    let mut crossings: Vec<(T, CandidateAction)> = Vec::new();

    for c in candidates {
        let tier = tier_of(&c);
        match classify_candidate(mode, tier, allow_cloud_escalation) {
            CandidateAction::Allow => same_tier.push((c, CandidateAction::Allow)),
            action @ CandidateAction::CrossTier { .. } => crossings.push((c, action)),
            CandidateAction::Skip => {}
        }
    }

    same_tier.extend(crossings);
    same_tier
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_allows_everything() {
        for tier in [
            EndpointTier::Local,
            EndpointTier::Cloud,
            EndpointTier::Unknown,
        ] {
            assert_eq!(
                classify_candidate(RouteMode::Auto, tier, false),
                CandidateAction::Allow
            );
            assert_eq!(
                classify_candidate(RouteMode::Auto, tier, true),
                CandidateAction::Allow
            );
        }
    }

    #[test]
    fn unknown_primary_always_allowed() {
        for mode in [
            RouteMode::Auto,
            RouteMode::AlwaysLocal,
            RouteMode::AlwaysCloud,
        ] {
            assert_eq!(
                classify_candidate(mode, EndpointTier::Unknown, false),
                CandidateAction::Allow
            );
        }
    }

    #[test]
    fn always_local_drops_cloud_without_escalation() {
        assert_eq!(
            classify_candidate(RouteMode::AlwaysLocal, EndpointTier::Cloud, false),
            CandidateAction::Skip
        );
        assert_eq!(
            classify_candidate(RouteMode::AlwaysLocal, EndpointTier::Local, false),
            CandidateAction::Allow
        );
    }

    #[test]
    fn always_local_gates_cloud_with_escalation() {
        assert_eq!(
            classify_candidate(RouteMode::AlwaysLocal, EndpointTier::Cloud, true),
            CandidateAction::CrossTier {
                requires_approval: true
            }
        );
    }

    #[test]
    fn always_cloud_degrades_local_ungated() {
        assert_eq!(
            classify_candidate(RouteMode::AlwaysCloud, EndpointTier::Local, false),
            CandidateAction::CrossTier {
                requires_approval: false
            }
        );
        assert_eq!(
            classify_candidate(RouteMode::AlwaysCloud, EndpointTier::Cloud, false),
            CandidateAction::Allow
        );
    }

    #[test]
    fn order_preserves_same_tier_order_and_appends_crossings_last() {
        // tiers: [Cloud, Local, Cloud, Local] under AlwaysLocal+escalate.
        // Locals (preferred) keep order; cloud crossings appended last.
        let cands = vec![
            ("c1", EndpointTier::Cloud),
            ("l1", EndpointTier::Local),
            ("c2", EndpointTier::Cloud),
            ("l2", EndpointTier::Local),
        ];
        let out = order_candidates(cands, RouteMode::AlwaysLocal, true, |(_, t)| *t);
        let names: Vec<&str> = out.iter().map(|((n, _), _)| *n).collect();
        assert_eq!(names, vec!["l1", "l2", "c1", "c2"]);
        // The appended crossings are approval-gated.
        assert_eq!(
            out[2].1,
            CandidateAction::CrossTier {
                requires_approval: true
            }
        );
    }

    #[test]
    fn order_in_auto_is_identity() {
        let cands = vec![
            ("a", EndpointTier::Cloud),
            ("b", EndpointTier::Local),
            ("c", EndpointTier::Unknown),
        ];
        let out = order_candidates(cands.clone(), RouteMode::Auto, false, |(_, t)| *t);
        let names: Vec<&str> = out.iter().map(|((n, _), _)| *n).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
        assert!(out.iter().all(|(_, a)| *a == CandidateAction::Allow));
    }
}
