//! R7-safe local/cloud route policy selector.
//!
//! A self-contained, prompt-blind module that turns two HARD signals — the
//! operator's [`RouteMode`] (an explicit user choice) and a candidate's
//! [`EndpointTier`] (a `base_url`-derived connectivity fact) — into a
//! [`CandidateAction`]. It never sees messages, tools, or the prompt; it
//! cannot classify task intent. Choosing local vs cloud is *infrastructure*
//! (R7 infrastructure layer), and the resulting candidate SET is still handed to the existing
//! failover engine which owns all retry / breaker / model-walk logic (R10 dumb
//! loop preserved — the harness never learns the route mode).
//!
//! In [`RouteMode::Auto`] every candidate is [`Allow`](CandidateAction::Allow)
//! in original order, so the policy is a no-op (byte-identical to pre-route
//! failover).

use crate::config::types::{LoadBalanceStrategy, ModelRouteConfig, RouteMode};
use crate::providers::model_catalog::EndpointKind;

/// Prompt-blind runtime signals about one candidate provider, consumed by the
/// load-balancing strategies. Produced by
/// [`LoadStats`](crate::providers::load_stats::LoadStats); kept here (not in
/// `load_stats`) so the ordering logic owns its own input type and stays
/// decoupled from the concurrent registry that fills it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LoadMetric {
    /// Requests currently in flight against this provider (lower is freer).
    pub in_flight: usize,
    /// Observed EWMA latency in microseconds; `0` = unsampled (tried first).
    pub latency_us: u64,
    /// Requests used in the trailing 60 seconds (weighted sliding-window count;
    /// filled by [`LoadStats`](crate::providers::load_stats::LoadStats) — the
    /// previous minute decays linearly across the boundary instead of snapping
    /// to zero). The failover layer folds this against the configured limit
    /// into the two derived fields below; the ordering logic itself never sees
    /// the limit (stays prompt- and config-blind, pure infrastructure).
    pub rpm_used: u32,
    /// Tokens used in the trailing 60 seconds (same weighted sliding window).
    pub tpm_used: u64,
    /// Rate-window utilisation in per-mille (‰): `max(rpm, tpm) / limit * 1000`,
    /// pre-computed by the failover layer. `0` when no limit is configured — so
    /// [`UsageBased`](crate::config::types::LoadBalanceStrategy::UsageBased)
    /// degrades to configured order. The sort key for `UsageBased`.
    pub utilization_permille: u16,
    /// Whether this provider has hit or exceeded any configured rate dimension
    /// (≥100% on rpm or tpm). Saturated providers are deprioritised to the back
    /// of their tier under every strategy (never dropped). `false` when no limit
    /// is set — so the gate is a no-op without `rate_limits`.
    pub over_limit: bool,
    /// Blended `input + output` price in **milli-USD per million tokens**
    /// (USD/Mtok × 1000) for this candidate's first model, looked up from the
    /// static [`crate::pricing`] rate card and folded in by the failover layer —
    /// the same "derived field, pre-computed outside the policy" contract as
    /// [`utilization_permille`](Self::utilization_permille). `0` = free local
    /// inference (an on-machine / self-hosted model that carries no rate card),
    /// which sorts *first* under
    /// [`CostAware`](crate::config::types::LoadBalanceStrategy::CostAware) so it
    /// is preferred; [`u64::MAX`] = unpriced *cloud* (a model absent from the
    /// table, whose cost is unknown rather than zero), which sorts *last* so it
    /// never ranks ahead of a confirmable price. The failover layer derives this
    /// split from each candidate's [`EndpointTier`]. The sort key for
    /// `CostAware`; ignored by every other strategy.
    pub price_per_mtok: u64,
    /// Whether this provider is currently sidelined by *health*: its circuit
    /// breaker is open, or it is parked inside a rate-limit pacing window.
    ///
    /// Deprioritised exactly like [`over_limit`](Self::over_limit) — to the back
    /// of its tier, never dropped, so the chain can still reach it as a last
    /// resort. Without this the breaker only acted *during* the walk, which
    /// meant a dead provider still led the chain and cost a probe round-trip on
    /// every request; worse, under `LeastBusy` / `LatencyAware` a dead provider
    /// looks *ideal* (nothing in flight, no latency samples) and sorts first.
    /// Filled by the failover layer from the shared breaker/cooldown registries;
    /// `false` in every unit test that supplies its own metric, so the ordering
    /// stays byte-identical there.
    pub cooling: bool,
}

impl LoadMetric {
    /// Whether this candidate should be pushed behind its healthy same-tier
    /// siblings — rate-saturated, unhealthy, or both.
    #[must_use]
    pub const fn is_sidelined(&self) -> bool {
        self.over_limit || self.cooling
    }
}

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
    #[must_use]
    pub fn from_config(cfg: &ModelRouteConfig) -> Self {
        Self {
            // rust-doctor-disable-next-line excessive-clone
            local_provider: cfg.local_provider.clone(),
            // rust-doctor-disable-next-line excessive-clone
            cloud_provider: cfg.cloud_provider.clone(),
        }
    }

    /// Whether `name` is the pinned provider for either tier.
    #[must_use]
    pub fn is_pinned(&self, name: &str) -> bool {
        self.local_provider.as_deref() == Some(name) || self.cloud_provider.as_deref() == Some(name)
    }

    /// Whether any pin is set (the common no-op fast path checks this first).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.local_provider.is_none() && self.cloud_provider.is_none()
    }
}

/// Operator's per-provider rate ceilings, lifted out of `[route].rate_limits`.
///
/// Owned here (not in `config`) so the ordering layer keeps its own input type
/// and the policy never depends on the config crate — the same decoupling
/// precedent as [`RouteTargets`] / [`LoadMetric`]. The failover layer calls
/// [`assess`](RateLimits::assess) to fold a provider's live window counts
/// against its limit into the two prompt-blind scalars the ordering reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RateLimits {
    /// provider name → `(rpm, tpm)`; `None` on a dimension means unbounded.
    by_provider: std::collections::BTreeMap<String, (Option<u32>, Option<u32>)>,
}

impl RateLimits {
    /// Lift the per-provider ceilings out of a `[route]` config snapshot.
    #[must_use]
    pub fn from_config(cfg: &ModelRouteConfig) -> Self {
        Self {
            by_provider: cfg
                .rate_limits
                .iter()
                // rust-doctor-disable-next-line excessive-clone
                .map(|(name, lim)| (name.clone(), (lim.rpm, lim.tpm)))
                .collect(),
        }
    }

    /// Whether any provider has a configured ceiling (the no-op fast path).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_provider.is_empty()
    }

    /// The configured `(rpm, tpm)` ceiling for `name`, if any. Diagnostic
    /// accessor for the `route_status` snapshot; the hot path only ever uses
    /// the folded output of [`assess`](Self::assess).
    #[must_use]
    pub fn ceiling(&self, name: &str) -> Option<(Option<u32>, Option<u32>)> {
        self.by_provider.get(name).copied()
    }

    /// Fold a provider's live window counts against its ceiling into
    /// `(utilisation‰, over_limit)`.
    ///
    /// Utilisation is the *max* of the rpm and tpm utilisation (the binding
    /// dimension); an unconfigured provider or dimension contributes 0. A
    /// provider is `over_limit` once any configured dimension reaches 100%. The
    /// per-mille value saturates at [`u16::MAX`] so a wildly-over provider still
    /// sorts last without overflow.
    #[must_use]
    pub fn assess(&self, name: &str, rpm_used: u32, tpm_used: u64) -> (u16, bool) {
        let Some(&(rpm, tpm)) = self.by_provider.get(name) else {
            return (0, false); // no ceiling → no signal
        };
        let rpm_permille = rpm
            .filter(|&l| l > 0)
            .map_or(0, |l| u64::from(rpm_used) * 1000 / u64::from(l));
        let tpm_permille = tpm
            .filter(|&l| l > 0)
            .map_or(0, |l| tpm_used * 1000 / u64::from(l));
        let permille = rpm_permille.max(tpm_permille);
        let over = permille >= 1000;
        (permille.min(u64::from(u16::MAX)) as u16, over)
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
            EndpointKind::Local => Self::Local,
            EndpointKind::Cloud => Self::Cloud,
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

/// The gate a candidate that SURVIVED ordering carries — [`CandidateAction`]
/// without the one variant no surviving candidate can hold.
///
/// Both ordering functions drop every [`Skip`](CandidateAction::Skip) before
/// returning ([`order_candidates`] and [`order_candidates_balanced`]), and the
/// failover walk drops it for the primary slot the same way. So a chain entry
/// typed `CandidateAction` had a fourth state that could not occur, and the
/// `route_status` renderer carried an arm for it that could never be reached —
/// an "always-true predicate" wearing a match arm's clothes.
///
/// [`retained`](Self::retained) is the ONLY conversion. A future
/// `CandidateAction` variant therefore stops the build there, in one place,
/// with the question "does this one survive ordering?" — instead of silently
/// picking up whichever arm a second, hand-written match happened to have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteGate {
    /// Dial it with no further gate.
    Allow,
    /// Dial it only after the cross-tier rule is satisfied — see
    /// [`CandidateAction::CrossTier`].
    CrossTier { requires_approval: bool },
}

impl RouteGate {
    /// The gate a retained candidate carries, or `None` when the policy drops
    /// the candidate outright.
    #[must_use]
    pub const fn retained(action: CandidateAction) -> Option<Self> {
        match action {
            CandidateAction::Allow => Some(Self::Allow),
            CandidateAction::CrossTier { requires_approval } => {
                Some(Self::CrossTier { requires_approval })
            }
            CandidateAction::Skip => None,
        }
    }
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
#[must_use]
pub const fn classify_candidate(
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
    // The default ordering is the load-balanced ordering under the no-op
    // `Ordered` strategy — keeping a single code path. `Ordered` never touches
    // the metric closure, so the byte-identical pre-balance order is preserved.
    order_candidates_balanced(
        candidates,
        mode,
        allow_cloud_escalation,
        targets,
        tier_of,
        name_of,
        LoadBalanceStrategy::Ordered,
        0,
        |_| LoadMetric::default(),
    )
}

/// Order and gate candidates exactly like [`order_candidates`], then apply a
/// load-balancing `strategy` *within* the non-pinned same-tier `Allow` group.
///
/// Layering is deliberate and order-preserving at every step:
/// 1. tier gating drops `Skip`, defers `CrossTier` crossings to the end;
/// 2. operator pins ([`RouteTargets`]) are promoted to the front and are
///    **never** reordered by the balancer (an explicit pin is a hard signal —
///    it leads even when rate-saturated);
/// 3. sidelined non-pinned candidates — rate-saturated (`over_limit`) or
///    unhealthy (`cooling`) — are split off and deprioritised to the back of the
///    tier (never dropped — the chain must not starve);
/// 4. the remaining fresh same-tier `Allow` candidates are reordered by
///    `strategy` using the prompt-blind [`LoadMetric`] from `metric_of` (and
///    `rr_base` for [`RoundRobin`](LoadBalanceStrategy::RoundRobin) rotation);
/// 5. cross-tier crossings are appended last, unchanged.
///
/// Under [`LoadBalanceStrategy::Ordered`] with no `rate_limits`, steps 3–4 are
/// no-ops, so the result is byte-identical to the pre-balance failover ordering.
#[allow(clippy::too_many_arguments)]
pub fn order_candidates_balanced<T, FT, FN, FM>(
    candidates: Vec<T>,
    mode: RouteMode,
    allow_cloud_escalation: bool,
    targets: &RouteTargets,
    tier_of: FT,
    name_of: FN,
    strategy: LoadBalanceStrategy,
    rr_base: u64,
    metric_of: FM,
) -> Vec<(T, CandidateAction)>
where
    FT: Fn(&T) -> EndpointTier,
    FN: Fn(&T) -> &str,
    FM: Fn(&str) -> LoadMetric,
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
    // `Vec::partition` preserves relative order within each side). An explicit
    // pin is a hard operator signal: it leads its tier even when rate-saturated
    // (pin beats the over-limit gate). The pinned group is exempt from balancing.
    // The failover walk's saturation gate honours the same exemption (judged by
    // the same `targets.is_pinned`), so a pin is never promoted here only to be
    // skipped at dial time — the pin yields to a dead circuit or a pacing
    // window (it does not exempt failure), never to capacity.
    let (pinned, unpinned): (Vec<(T, CandidateAction)>, Vec<(T, CandidateAction)>) =
        if targets.is_empty() {
            (Vec::new(), same_tier)
        } else {
            same_tier
                .into_iter()
                .partition(|(c, _)| targets.is_pinned(name_of(c)))
        };

    // Sideline gate (strategy-agnostic): deprioritise rate-saturated *and*
    // unhealthy providers to the back of the tier — appended after the balanced
    // fresh group, never dropped, so the chain can still fall back to a
    // throttled or cooling endpoint rather than starve. A no-op when neither
    // signal is present (every `is_sidelined` is `false` → `sidelined` is empty
    // → byte-identical to pre-gate ordering). Stable: `partition` preserves
    // configured order within the sidelined group.
    let (fresh, sidelined): (Vec<(T, CandidateAction)>, Vec<(T, CandidateAction)>) = unpinned
        .into_iter()
        .partition(|(c, _)| !metric_of(name_of(c)).is_sidelined());

    let mut out: Vec<(T, CandidateAction)> = pinned;
    out.extend(balance_group(
        fresh, strategy, rr_base, &metric_of, &name_of,
    ));
    out.extend(sidelined);
    out.extend(crossings);
    out
}

/// Reorder one same-tier `Allow` group by `strategy`. Stable for the sort-based
/// strategies (ties keep configured order); a pure rotation for round-robin.
fn balance_group<T, FN, FM>(
    group: Vec<(T, CandidateAction)>,
    strategy: LoadBalanceStrategy,
    rr_base: u64,
    metric_of: &FM,
    name_of: &FN,
) -> Vec<(T, CandidateAction)>
where
    FN: Fn(&T) -> &str,
    FM: Fn(&str) -> LoadMetric,
{
    if group.len() <= 1 {
        return group;
    }
    match strategy {
        LoadBalanceStrategy::Ordered => group,
        LoadBalanceStrategy::RoundRobin => {
            let mut g = group;
            let k = (rr_base % g.len() as u64) as usize;
            g.rotate_left(k);
            g
        }
        LoadBalanceStrategy::LeastBusy => {
            sort_by_metric(group, metric_of, name_of, |m| m.in_flight as u64)
        }
        // `0` (unsampled) sorts first, so a fresh endpoint is tried before
        // already-measured ones and gains a latency reading.
        LoadBalanceStrategy::LatencyAware => {
            sort_by_metric(group, metric_of, name_of, |m| m.latency_us)
        }
        // Lowest rate-window utilisation first — spread load toward whoever has
        // the most remaining headroom. `0‰` (no limit / idle) sorts first.
        LoadBalanceStrategy::UsageBased => sort_by_metric(group, metric_of, name_of, |m| {
            u64::from(m.utilization_permille)
        }),
        // Cheapest first — lowest blended input+output price (milli-USD/Mtok).
        // `0` (free local / self-hosted) sorts first, so effectively-free
        // on-machine inference is preferred over any paid cloud endpoint;
        // `u64::MAX` (unpriced cloud, cost unknown) sorts last so it never
        // out-ranks a confirmable price. The failover layer sets both.
        LoadBalanceStrategy::CostAware => {
            sort_by_metric(group, metric_of, name_of, |m| m.price_per_mtok)
        }
    }
}

/// A `[route]` setting that is present but cannot do anything, with the reason.
///
/// Every entry here is a knob the operator (or the model, via `update_config`)
/// deliberately set and which the engine then silently ignores — the failure
/// mode being "my routing configuration did nothing", with no error anywhere to
/// explain it. `is_pinned` matches by name only, so a typo'd or removed
/// provider simply never matches; a ceiling keyed to a provider that is not
/// configured is never assessed; a local pin naming a cloud endpoint promotes a
/// candidate the tier gate has already moved to the back.
///
/// Reported at boot (WARN, one line per problem) and in the `route_status`
/// snapshot, so the same answer reaches the operator and the model. Purely
/// descriptive — nothing here changes a routing decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteProblem {
    /// `[route]` field the problem is about (`local_provider`, `rate_limits.x`…).
    pub field: String,
    /// What is wrong, in one sentence.
    pub detail: String,
}

/// Find every inert `[route]` setting, given the configured provider names and
/// each one's endpoint tier.
///
/// Pure over already-gathered facts (same contract as
/// [`crate::routing::overlay_route`]): the caller supplies the provider picture
/// it can see, and the rules live in one place.
#[must_use]
pub fn route_problems(
    cfg: &ModelRouteConfig,
    tiers: &std::collections::HashMap<String, EndpointTier>,
) -> Vec<RouteProblem> {
    let mut out = Vec::new();
    let known = |name: &str| tiers.contains_key(name);
    let names = || {
        let mut n: Vec<&str> = tiers.keys().map(String::as_str).collect();
        n.sort_unstable();
        n.join(", ")
    };

    for (field, pin, want) in [
        (
            "local_provider",
            cfg.local_provider.as_deref(),
            EndpointTier::Local,
        ),
        (
            "cloud_provider",
            cfg.cloud_provider.as_deref(),
            EndpointTier::Cloud,
        ),
    ] {
        let Some(pin) = pin else { continue };
        if !known(pin) {
            out.push(RouteProblem {
                field: field.to_string(),
                detail: format!(
                    "pins '{pin}', which is not defined under [providers] — the pin never \
                     matches a candidate and is ignored. Configured providers: {}",
                    names()
                ),
            });
        } else if tiers.get(pin) != Some(&want) {
            out.push(RouteProblem {
                field: field.to_string(),
                detail: format!(
                    "pins '{pin}', whose endpoint is not {}. A pin only leads within its own \
                     tier, so under a tier-shaping mode this promotes a candidate the tier \
                     gate has already deferred.",
                    if want == EndpointTier::Local {
                        "local"
                    } else {
                        "cloud"
                    }
                ),
            });
        }
    }

    for (name, limit) in &cfg.rate_limits {
        if !known(name) {
            out.push(RouteProblem {
                field: format!("rate_limits.{name}"),
                detail: format!(
                    "sets a ceiling for '{name}', which is not defined under [providers] — no \
                     request is ever counted against it. Configured providers: {}",
                    names()
                ),
            });
        } else if limit.rpm.is_none() && limit.tpm.is_none() {
            out.push(RouteProblem {
                field: format!("rate_limits.{name}"),
                detail: "has neither rpm nor tpm set, so both dimensions are unbounded — the \
                         entry has no effect."
                    .to_string(),
            });
        }
    }

    if cfg.allow_cloud_escalation && cfg.mode != RouteMode::AlwaysLocal {
        out.push(RouteProblem {
            field: "allow_cloud_escalation".to_string(),
            detail: format!(
                "is only consulted in mode 'always_local'; the current mode is '{}', where \
                 cloud candidates need no escalation.",
                match cfg.mode {
                    RouteMode::Auto => "auto",
                    RouteMode::AlwaysCloud => "always_cloud",
                    RouteMode::AlwaysLocal => "always_local",
                }
            ),
        });
    }

    out
}

/// Stable-sort a candidate group by an integer key derived from each
/// candidate's [`LoadMetric`]. The key is computed once per candidate (not per
/// comparison) so `metric_of` is called exactly `len` times.
fn sort_by_metric<T, FN, FM, K>(
    group: Vec<(T, CandidateAction)>,
    metric_of: &FM,
    name_of: &FN,
    key: K,
) -> Vec<(T, CandidateAction)>
where
    FN: Fn(&T) -> &str,
    FM: Fn(&str) -> LoadMetric,
    K: Fn(LoadMetric) -> u64,
{
    let mut keyed: Vec<(u64, (T, CandidateAction))> = group
        .into_iter()
        .map(|c| {
            let k = key(metric_of(name_of(&c.0)));
            (k, c)
        })
        .collect();
    keyed.sort_by_key(|(k, _)| *k); // stable: equal keys keep configured order
    keyed.into_iter().map(|(_, c)| c).collect()
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

    // --- load-balance strategy tests --------------------------------------

    /// Helper: order three same-tier locals under `strategy`, with `metrics`
    /// mapping name → (in_flight, latency_us), and return the resulting order.
    fn balanced_names(
        strategy: LoadBalanceStrategy,
        rr_base: u64,
        metrics: &[(&str, usize, u64)],
    ) -> Vec<&'static str> {
        let cands = vec![
            ("a", EndpointTier::Local),
            ("b", EndpointTier::Local),
            ("c", EndpointTier::Local),
        ];
        let lookup = metrics.to_vec();
        let out = order_candidates_balanced(
            cands,
            RouteMode::Auto,
            false,
            &RouteTargets::default(),
            |(_, t)| *t,
            |(n, _)| *n,
            strategy,
            rr_base,
            |name| {
                lookup
                    .iter()
                    .find(|(n, _, _)| *n == name)
                    .map(|(_, f, l)| LoadMetric {
                        in_flight: *f,
                        latency_us: *l,
                        ..Default::default()
                    })
                    .unwrap_or_default()
            },
        );
        out.iter().map(|((n, _), _)| *n).collect()
    }

    #[test]
    fn ordered_strategy_is_identity() {
        let names = balanced_names(LoadBalanceStrategy::Ordered, 7, &[]);
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn least_busy_prefers_fewest_in_flight() {
        // b has 0 in-flight, a has 5, c has 2 → order b, c, a.
        let names = balanced_names(
            LoadBalanceStrategy::LeastBusy,
            0,
            &[("a", 5, 0), ("b", 0, 0), ("c", 2, 0)],
        );
        assert_eq!(names, vec!["b", "c", "a"]);
    }

    #[test]
    fn least_busy_ties_keep_configured_order() {
        // All equal → stable sort preserves a, b, c.
        let names = balanced_names(
            LoadBalanceStrategy::LeastBusy,
            0,
            &[("a", 3, 0), ("b", 3, 0), ("c", 3, 0)],
        );
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn latency_aware_prefers_lowest_and_samples_unknown_first() {
        // a=900us, b=unsampled(0), c=300us → b (unknown, tried first), c, a.
        let names = balanced_names(
            LoadBalanceStrategy::LatencyAware,
            0,
            &[("a", 0, 900), ("b", 0, 0), ("c", 0, 300)],
        );
        assert_eq!(names, vec!["b", "c", "a"]);
    }

    #[test]
    fn round_robin_rotates_first_choice_by_counter() {
        assert_eq!(
            balanced_names(LoadBalanceStrategy::RoundRobin, 0, &[]),
            vec!["a", "b", "c"]
        );
        assert_eq!(
            balanced_names(LoadBalanceStrategy::RoundRobin, 1, &[]),
            vec!["b", "c", "a"]
        );
        assert_eq!(
            balanced_names(LoadBalanceStrategy::RoundRobin, 2, &[]),
            vec!["c", "a", "b"]
        );
        // Wraps modulo the group length.
        assert_eq!(
            balanced_names(LoadBalanceStrategy::RoundRobin, 3, &[]),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn balancing_never_reorders_pins() {
        // Pin "c" → it leads regardless of load; the unpinned remainder (a, b)
        // is then least-busy ordered (b before a).
        let cands = vec![
            ("a", EndpointTier::Local),
            ("b", EndpointTier::Local),
            ("c", EndpointTier::Local),
        ];
        let targets = RouteTargets {
            local_provider: Some("c".to_string()),
            cloud_provider: None,
        };
        let out = order_candidates_balanced(
            cands,
            RouteMode::Auto,
            false,
            &targets,
            |(_, t)| *t,
            |(n, _)| *n,
            LoadBalanceStrategy::LeastBusy,
            0,
            |name| match name {
                "a" => LoadMetric {
                    in_flight: 9,
                    ..Default::default()
                },
                "b" => LoadMetric {
                    in_flight: 1,
                    ..Default::default()
                },
                _ => LoadMetric::default(),
            },
        );
        let names: Vec<&str> = out.iter().map(|((n, _), _)| *n).collect();
        assert_eq!(names, vec!["c", "b", "a"]);
    }

    // --- usage-based strategy + over-limit gate tests -----------------------

    /// Order three same-tier locals where `metrics` maps name → (utilisation‰,
    /// over_limit), under `strategy`, with optional `pins`.
    fn usage_names(
        strategy: LoadBalanceStrategy,
        pins: &RouteTargets,
        metrics: &[(&str, u16, bool)],
    ) -> Vec<&'static str> {
        let cands = vec![
            ("a", EndpointTier::Local),
            ("b", EndpointTier::Local),
            ("c", EndpointTier::Local),
        ];
        let lookup = metrics.to_vec();
        let out = order_candidates_balanced(
            cands,
            RouteMode::Auto,
            false,
            pins,
            |(_, t)| *t,
            |(n, _)| *n,
            strategy,
            0,
            |name| {
                lookup
                    .iter()
                    .find(|(n, _, _)| *n == name)
                    .map(|(_, u, o)| LoadMetric {
                        utilization_permille: *u,
                        over_limit: *o,
                        ..Default::default()
                    })
                    .unwrap_or_default()
            },
        );
        out.iter().map(|((n, _), _)| *n).collect()
    }

    #[test]
    fn usage_based_prefers_lowest_utilisation() {
        // a=700‰, b=100‰, c=400‰ → ascending headroom: b, c, a.
        let names = usage_names(
            LoadBalanceStrategy::UsageBased,
            &RouteTargets::default(),
            &[("a", 700, false), ("b", 100, false), ("c", 400, false)],
        );
        assert_eq!(names, vec!["b", "c", "a"]);
    }

    #[test]
    fn over_limit_deprioritised_to_back_under_any_strategy() {
        // b is saturated → pushed last even though Ordered would keep a,b,c.
        let names = usage_names(
            LoadBalanceStrategy::Ordered,
            &RouteTargets::default(),
            &[("a", 0, false), ("b", 1200, true), ("c", 0, false)],
        );
        assert_eq!(names, vec!["a", "c", "b"]);
    }

    #[test]
    fn unhealthy_candidates_are_deprioritised_like_saturated_ones() {
        // The breaker used to act only *during* the walk, so a dead provider
        // still led the chain and cost a probe round-trip every request — and
        // under LeastBusy/LatencyAware it looked ideal (nothing in flight, no
        // latency samples) and sorted first.
        let cands = vec![
            ("a", EndpointTier::Local),
            ("b", EndpointTier::Local),
            ("c", EndpointTier::Local),
        ];
        let out = order_candidates_balanced(
            cands,
            RouteMode::Auto,
            false,
            &RouteTargets::default(),
            |(_, t)| *t,
            |(n, _)| *n,
            LoadBalanceStrategy::Ordered,
            0,
            |name| LoadMetric {
                cooling: name == "a",
                ..Default::default()
            },
        );
        let names: Vec<&str> = out.iter().map(|((n, _), _)| *n).collect();
        // Never dropped — the chain must still be able to reach it last.
        assert_eq!(names, vec!["b", "c", "a"]);
    }

    #[test]
    fn pin_beats_over_limit_gate() {
        // Pin "b" though it is saturated → it still leads; a,c fresh follow.
        // The walk's saturation gate exempts a pinned candidate for the same
        // reason (see `a_pinned_provider_is_dialed_even_when_saturated` in the
        // failover tests) — this ordering promise and the dial-time gate are
        // one rule, not two.
        let targets = RouteTargets {
            local_provider: Some("b".to_string()),
            cloud_provider: None,
        };
        let names = usage_names(
            LoadBalanceStrategy::Ordered,
            &targets,
            &[("a", 0, false), ("b", 1500, true), ("c", 0, false)],
        );
        assert_eq!(names, vec!["b", "a", "c"]);
    }

    #[test]
    fn no_limits_is_byte_identical_under_usage_based() {
        // Every metric 0‰/not-over → UsageBased degrades to configured order.
        let names = usage_names(
            LoadBalanceStrategy::UsageBased,
            &RouteTargets::default(),
            &[],
        );
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn rate_limits_assess_folds_binding_dimension() {
        let cfg = ModelRouteConfig {
            rate_limits: [(
                "p".to_string(),
                crate::config::types::ProviderRateLimit {
                    rpm: Some(100),
                    tpm: Some(1000),
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let limits = RateLimits::from_config(&cfg);
        assert!(!limits.is_empty());
        // rpm 50/100 = 500‰, tpm 200/1000 = 200‰ → binding dim = 500‰, not over.
        assert_eq!(limits.assess("p", 50, 200), (500, false));
        // tpm at the ceiling → over_limit.
        assert_eq!(limits.assess("p", 0, 1000), (1000, true));
        // Unconfigured provider → no signal.
        assert_eq!(limits.assess("other", 9999, 9999), (0, false));
    }

    // --- cost-aware strategy tests -----------------------------------------

    /// Order three same-tier locals where `prices` maps name → milli-USD/Mtok,
    /// under [`LoadBalanceStrategy::CostAware`].
    fn cost_names(prices: &[(&str, u64)]) -> Vec<&'static str> {
        let cands = vec![
            ("a", EndpointTier::Local),
            ("b", EndpointTier::Local),
            ("c", EndpointTier::Local),
        ];
        let lookup = prices.to_vec();
        let out = order_candidates_balanced(
            cands,
            RouteMode::Auto,
            false,
            &RouteTargets::default(),
            |(_, t)| *t,
            |(n, _)| *n,
            LoadBalanceStrategy::CostAware,
            0,
            |name| {
                lookup
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, p)| LoadMetric {
                        price_per_mtok: *p,
                        ..Default::default()
                    })
                    .unwrap_or_default()
            },
        );
        out.iter().map(|((n, _), _)| *n).collect()
    }

    #[test]
    fn cost_aware_prefers_cheapest() {
        // a=90k, b=30k, c=60k milli-USD/Mtok → ascending price: b, c, a.
        let names = cost_names(&[("a", 90_000), ("b", 30_000), ("c", 60_000)]);
        assert_eq!(names, vec!["b", "c", "a"]);
    }

    #[test]
    fn cost_aware_unpriced_sorts_first() {
        // b unpriced (0 = free local) leads even though a/c are priced.
        let names = cost_names(&[("a", 50_000), ("b", 0), ("c", 20_000)]);
        assert_eq!(names, vec!["b", "c", "a"]);
    }

    #[test]
    fn cost_aware_unknown_cost_sentinel_sorts_last() {
        // `u64::MAX` is the failover layer's "unpriced cloud, cost unknown"
        // sentinel: it must rank behind every confirmable price rather than
        // ahead of them (the bug where an unpriced cloud model looked free).
        let names = cost_names(&[("a", 50_000), ("b", u64::MAX), ("c", 20_000)]);
        assert_eq!(names, vec!["c", "a", "b"]);
    }

    #[test]
    fn cost_aware_ties_keep_configured_order() {
        // All equal price → stable sort preserves a, b, c.
        let names = cost_names(&[("a", 10_000), ("b", 10_000), ("c", 10_000)]);
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    // --- inert-config diagnostics ------------------------------------------

    fn tiers(entries: &[(&str, EndpointTier)]) -> std::collections::HashMap<String, EndpointTier> {
        entries
            .iter()
            .map(|(n, t)| ((*n).to_string(), *t))
            .collect()
    }

    #[test]
    fn a_clean_route_config_reports_no_problems() {
        let cfg = ModelRouteConfig {
            mode: RouteMode::AlwaysLocal,
            allow_cloud_escalation: true,
            local_provider: Some("ollama".to_string()),
            cloud_provider: Some("anthropic".to_string()),
            ..Default::default()
        };
        let map = tiers(&[
            ("ollama", EndpointTier::Local),
            ("anthropic", EndpointTier::Cloud),
        ]);
        assert!(route_problems(&cfg, &map).is_empty());
    }

    #[test]
    fn a_pin_naming_an_unconfigured_provider_is_reported_with_the_valid_names() {
        // The silent failure this exists for: `is_pinned` matches by name, so a
        // typo'd or since-deleted provider simply never matches and the only
        // symptom is "my routing configuration did nothing".
        let cfg = ModelRouteConfig {
            local_provider: Some("olama".to_string()),
            ..Default::default()
        };
        let map = tiers(&[("ollama", EndpointTier::Local)]);
        let problems = route_problems(&cfg, &map);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].field, "local_provider");
        assert!(problems[0].detail.contains("olama"));
        assert!(problems[0].detail.contains("ollama"), "lists what is valid");
    }

    #[test]
    fn a_pin_on_the_wrong_tier_is_reported() {
        // A pin only leads within its own tier, so pinning a cloud endpoint as
        // the *local* preference promotes a candidate the tier gate has
        // already deferred.
        let cfg = ModelRouteConfig {
            local_provider: Some("anthropic".to_string()),
            ..Default::default()
        };
        let map = tiers(&[("anthropic", EndpointTier::Cloud)]);
        let problems = route_problems(&cfg, &map);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].field, "local_provider");
        assert!(problems[0].detail.contains("not local"));
    }

    #[test]
    fn inert_rate_limit_entries_are_reported() {
        let cfg = ModelRouteConfig {
            rate_limits: [
                (
                    "ghost".to_string(),
                    crate::config::types::ProviderRateLimit {
                        rpm: Some(60),
                        tpm: None,
                    },
                ),
                (
                    "ollama".to_string(),
                    crate::config::types::ProviderRateLimit {
                        rpm: None,
                        tpm: None,
                    },
                ),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let map = tiers(&[("ollama", EndpointTier::Local)]);
        let problems = route_problems(&cfg, &map);
        let fields: Vec<&str> = problems.iter().map(|p| p.field.as_str()).collect();
        assert_eq!(fields, vec!["rate_limits.ghost", "rate_limits.ollama"]);
        // A ceiling on an unconfigured provider counts nothing; an entry with
        // neither dimension set bounds nothing.
        assert!(problems[0].detail.contains("not defined under [providers]"));
        assert!(problems[1].detail.contains("neither rpm nor tpm"));
    }

    #[test]
    fn escalation_outside_always_local_is_reported_as_ignored() {
        let cfg = ModelRouteConfig {
            mode: RouteMode::Auto,
            allow_cloud_escalation: true,
            ..Default::default()
        };
        let problems = route_problems(&cfg, &tiers(&[]));
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].field, "allow_cloud_escalation");
        assert!(problems[0].detail.contains("always_local"));

        // …and is silent in the mode that actually consults it.
        let cfg = ModelRouteConfig {
            mode: RouteMode::AlwaysLocal,
            allow_cloud_escalation: true,
            ..Default::default()
        };
        assert!(route_problems(&cfg, &tiers(&[])).is_empty());
    }

    #[test]
    fn cost_aware_all_unpriced_is_byte_identical() {
        // No candidate priced → every key 0 → configured order preserved.
        let names = cost_names(&[]);
        assert_eq!(names, vec!["a", "b", "c"]);
    }
}
