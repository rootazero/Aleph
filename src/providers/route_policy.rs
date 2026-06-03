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

use crate::config::types::{ModelRouteConfig, RouteMode};
use crate::providers::model_catalog::EndpointKind;

/// Operator's explicit per-tier provider preference — "use *this* local /
/// *this* cloud provider", chosen by name from the already-configured
/// `[providers]` (the panel never redefines a provider, it just picks one).
///
/// Both `None` (default) means no promotion: the configured candidate order is
/// the route, byte-identical to pre-selection failover. A hard signal like
/// [`RouteMode`] — names only, never the prompt (R7 preserved).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteTargets {
    /// Preferred local provider name, promoted to the front of the local tier.
    pub local_provider: Option<String>,
    /// Preferred cloud provider name, promoted to the front of the cloud tier.
    pub cloud_provider: Option<String>,
}

impl RouteTargets {
    /// Lift the two pins out of a `[route]` config snapshot.
    pub fn from_config(cfg: &ModelRouteConfig) -> Self {
        Self {
            local_provider: cfg.local_provider.clone(),
            cloud_provider: cfg.cloud_provider.clone(),
        }
    }

    /// Whether `name` is the pinned provider for either tier.
    pub fn is_pinned(&self, name: &str) -> bool {
        self.local_provider.as_deref() == Some(name) || self.cloud_provider.as_deref() == Some(name)
    }

    /// Whether any pin is set (the common no-op fast path checks this first).
    pub fn is_empty(&self) -> bool {
        self.local_provider.is_none() && self.cloud_provider.is_none()
    }
}

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

/// Order and gate a candidate list under `mode`, honouring the operator's
/// provider pins.
///
/// Partitions into `Allow` candidates (preferred tier) followed by `CrossTier`
/// crossings appended LAST, dropping every `Skip`. Within the `Allow` group a
/// pinned provider ([`RouteTargets`]) is *stably promoted* to the front so the
/// active route dials the operator's chosen local/cloud endpoint first; all
/// other relative order is preserved. Each retained candidate is paired with
/// the [`CandidateAction`] the failover walk must enforce. Generic over `T` so
/// it is unit-testable without real providers.
///
/// `tier_of` extracts a candidate's [`EndpointTier`]; `name_of` its provider
/// name (matched against the pins). When `targets` is empty this is identical
/// to the unpinned ordering.
pub fn order_candidates<T, FT, FN>(
    candidates: Vec<T>,
    mode: RouteMode,
    allow_cloud_escalation: bool,
    targets: &RouteTargets,
    tier_of: FT,
    name_of: FN,
) -> Vec<(T, CandidateAction)>
where
    FT: Fn(&T) -> EndpointTier,
    FN: Fn(&T) -> &str,
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

    // Promote pinned providers to the front of the active tier (stable —
    // `Vec::partition` preserves relative order within each side). Skipped
    // entirely when no pin is set, keeping the no-pin path allocation-light.
    if !targets.is_empty() {
        let (pinned, rest): (Vec<_>, Vec<_>) = same_tier
            .into_iter()
            .partition(|(c, _)| targets.is_pinned(name_of(c)));
        same_tier = pinned;
        same_tier.extend(rest);
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
        let out = order_candidates(
            cands,
            RouteMode::AlwaysLocal,
            true,
            &RouteTargets::default(),
            |(_, t)| *t,
            |(n, _)| *n,
        );
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
        let out = order_candidates(
            cands.clone(),
            RouteMode::Auto,
            false,
            &RouteTargets::default(),
            |(_, t)| *t,
            |(n, _)| *n,
        );
        let names: Vec<&str> = out.iter().map(|((n, _), _)| *n).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
        assert!(out.iter().all(|(_, a)| *a == CandidateAction::Allow));
    }

    #[test]
    fn pin_promotes_chosen_provider_within_tier() {
        // Locals [l1, l2, l3]; pin l3 → l3 jumps to the front, others hold order.
        let cands = vec![
            ("l1", EndpointTier::Local),
            ("l2", EndpointTier::Local),
            ("l3", EndpointTier::Local),
        ];
        let targets = RouteTargets {
            local_provider: Some("l3".to_string()),
            cloud_provider: None,
        };
        let out = order_candidates(
            cands,
            RouteMode::AlwaysLocal,
            false,
            &targets,
            |(_, t)| *t,
            |(n, _)| *n,
        );
        let names: Vec<&str> = out.iter().map(|((n, _), _)| *n).collect();
        assert_eq!(names, vec!["l3", "l1", "l2"]);
    }

    #[test]
    fn pin_in_auto_promotes_both_tier_leaders_stably() {
        // Auto keeps every candidate; pins bring the chosen local + cloud to the
        // front in their original relative order, the rest trailing unchanged.
        let cands = vec![
            ("c1", EndpointTier::Cloud),
            ("l1", EndpointTier::Local),
            ("c2", EndpointTier::Cloud),
            ("l2", EndpointTier::Local),
        ];
        let targets = RouteTargets {
            local_provider: Some("l2".to_string()),
            cloud_provider: Some("c2".to_string()),
        };
        let out = order_candidates(
            cands,
            RouteMode::Auto,
            false,
            &targets,
            |(_, t)| *t,
            |(n, _)| *n,
        );
        let names: Vec<&str> = out.iter().map(|((n, _), _)| *n).collect();
        // c2 and l2 are pinned → promoted (stable: c2 precedes l2 as in input),
        // then the unpinned c1, l1 follow in original order.
        assert_eq!(names, vec!["c2", "l2", "c1", "l1"]);
    }

    #[test]
    fn empty_targets_is_byte_identical_ordering() {
        let cands = vec![
            ("a", EndpointTier::Local),
            ("b", EndpointTier::Local),
            ("c", EndpointTier::Cloud),
        ];
        let pinned = order_candidates(
            cands.clone(),
            RouteMode::Auto,
            false,
            &RouteTargets::default(),
            |(_, t)| *t,
            |(n, _)| *n,
        );
        let names: Vec<&str> = pinned.iter().map(|((n, _), _)| *n).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }
}
