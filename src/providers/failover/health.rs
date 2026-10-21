//! Circuit-breaker and per-(provider, model) rate-limit cooldown registries.
//!
//! Split out of `failover` so the shared breaker/cooldown state lives apart
//! from the provider walk in [`super::provider`] that drives it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::sync_primitives::Arc;

static GLOBAL_FAILOVER_HEALTH: CapabilitySlot<FailoverHealth> = CapabilitySlot::new(
    "providers/failover-health",
    MissingSemantics::ConsumerDecides,
);

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn failover_health_slot() -> &'static dyn SlotStatus {
    &GLOBAL_FAILOVER_HEALTH
}

/// Register the breaker state used by the boot-built failover chain.
///
/// The slot is process-wide and install-once. Cloning `FailoverHealth` shares
/// the same underlying map, so gateway recovery paths observe the exact state
/// that the provider chain mutates.
pub fn register_global_health(health: FailoverHealth) {
    let _ = GLOBAL_FAILOVER_HEALTH.install(health);
}

/// Return the boot-built failover health, if provider wiring has run.
pub fn global_health() -> Option<&'static FailoverHealth> {
    GLOBAL_FAILOVER_HEALTH.get()
}

/// Circuit-breaker state for a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Healthy — requests flow through.
    Closed,
    /// Tripped — requests skip this provider until the cooldown expires.
    Open,
    /// Cooldown expired — the next request is a single probe.
    HalfOpen,
}

/// Per-provider health tracked by the circuit breaker.
#[derive(Debug, Clone)]
pub(crate) struct HealthState {
    pub(crate) circuit: CircuitState,
    pub(crate) last_failure: Option<Instant>,
    pub(crate) failure_count: u32,
    pub(crate) last_error: Option<String>,
    pub(crate) cooldown: Duration,
}

impl Default for HealthState {
    fn default() -> Self {
        Self {
            circuit: CircuitState::Closed,
            last_failure: None,
            failure_count: 0,
            last_error: None,
            cooldown: Duration::from_secs(300),
        }
    }
}

/// Per-provider circuit-breaker state, keyed by provider name.
///
/// Cloning shares the same underlying map (`Arc`): one provider's outage
/// recorded by the global chain is immediately visible to every per-agent
/// chain that was built with the same `FailoverHealth`.
#[derive(Clone)]
pub struct FailoverHealth(pub(crate) Arc<RwLock<HashMap<String, HealthState>>>);

impl Default for FailoverHealth {
    fn default() -> Self {
        Self(Arc::new(RwLock::new(HashMap::new())))
    }
}

/// Read-only view of one provider's circuit-breaker state, surfaced to
/// recovery and diagnostic callers.
#[derive(Debug, Clone)]
pub struct ProviderHealthView {
    /// Provider name (the circuit-breaker key).
    pub provider: String,
    /// Circuit state: `"closed"`, `"open"`, or `"half_open"`.
    pub circuit: &'static str,
    /// Consecutive failures recorded since the last success.
    pub failure_count: u32,
    /// The error that drove the most recent failure, if any.
    pub last_error: Option<String>,
    /// Seconds until an `Open` circuit allows a probe; `0` means the next
    /// request probes immediately (effectively half-open). `None` when the
    /// circuit is not open.
    pub cooldown_remaining_secs: Option<u64>,
}

impl FailoverHealth {
    /// Close `name`'s circuit and clear its failure count / cooldown, returning
    /// whether an entry existed.
    ///
    /// The operator's escape hatch from a `Permanent` trip. A revoked key opens
    /// the breaker on the *first* strike with the full `MAX_COOLDOWN` window
    /// (`super::MAX_COOLDOWN`, 10 minutes), which is right while the key is
    /// still revoked and wrong the moment it is rotated: without this, "Test
    /// connection" probes the provider successfully, reports green, and the next
    /// ten minutes of real requests keep skipping it with only a `debug!` line.
    /// The probe itself deliberately never mutates state, so the reset is the
    /// caller's explicit act.
    ///
    /// Deliberately does **not** touch the rate-limit cooldowns
    /// ([`ModelCooldown`] / [`ProviderCooldown`]): a successful probe proves the
    /// endpoint answers, not that a throttling window has elapsed.
    pub async fn reset(&self, name: &str) -> bool {
        self.0.write().await.remove(name).is_some()
    }

    /// Open `name`'s circuit directly, as if a failure had just tripped it,
    /// with `cooldown` still to run from now.
    ///
    /// Test-only (`gateway::health_prober`, `failover::tests`): the prober's
    /// recovery policy is unit-tested against a genuinely open breaker without
    /// driving a whole failing provider walk first.
    ///
    /// The cooldown is a PARAMETER because the two Open states behave
    /// differently at the gate: with time left, `circuit_allows` refuses and
    /// mutates nothing; with `Duration::ZERO` (or any elapsed window) it
    /// performs the `Open -> HalfOpen` transition that spends the probe. A test
    /// pinning "looking does not spend the probe" is unfailable unless it
    /// constructs the second state.
    #[cfg(test)]
    pub(crate) async fn open_for_test(&self, name: &str, cooldown: Duration) {
        self.0.write().await.insert(
            name.to_string(),
            HealthState {
                circuit: CircuitState::Open,
                last_failure: Some(Instant::now()),
                failure_count: 1,
                last_error: None,
                cooldown,
            },
        );
    }

    /// Snapshot every tracked provider's breaker state, name-sorted.
    /// Diagnostic only — the hot path keeps using `circuit_allows`.
    pub async fn snapshot(&self) -> Vec<ProviderHealthView> {
        let map = self.0.read().await;
        let mut out: Vec<ProviderHealthView> = map
            .iter()
            .map(|(name, st)| {
                let circuit = match st.circuit {
                    CircuitState::Closed => "closed",
                    CircuitState::Open => "open",
                    CircuitState::HalfOpen => "half_open",
                };
                let cooldown_remaining_secs = (st.circuit == CircuitState::Open).then(|| {
                    st.last_failure
                        .map_or(0, |at| st.cooldown.saturating_sub(at.elapsed()).as_secs())
                });
                ProviderHealthView {
                    // rust-doctor-disable-next-line excessive-clone
                    provider: name.clone(),
                    circuit,
                    failure_count: st.failure_count,
                    // rust-doctor-disable-next-line excessive-clone
                    last_error: st.last_error.clone(),
                    cooldown_remaining_secs,
                }
            })
            .collect();
        out.sort_by(|a, b| a.provider.cmp(&b.provider));
        out
    }
}

/// Per-(provider, model) rate-limit cooldown.
///
/// A model that returned a *model-specific* 429 is sidelined until its cooldown
/// expires, so the walk prefers a healthy sibling model (or provider) instead
/// of re-hitting the throttled one. Distinct from the provider-level circuit
/// breaker ([`FailoverHealth`]): a rate-limited model does **not** trip the
/// whole provider — its siblings stay live. Cloning shares the same map
/// (`Arc`), so a throttle recorded by the global chain is visible to every
/// per-hint override (scoped exactly like `FailoverHealth`).
#[derive(Clone)]
pub struct ModelCooldown(Arc<RwLock<HashMap<(String, String), Instant>>>);

impl Default for ModelCooldown {
    fn default() -> Self {
        Self(Arc::new(RwLock::new(HashMap::new())))
    }
}

impl ModelCooldown {
    /// Sideline `(provider, model)` for `dur`.
    pub(crate) async fn cool(&self, provider: &str, model: &str, dur: Duration) {
        let until = Instant::now() + dur;
        self.0
            .write()
            .await
            .insert((provider.to_string(), model.to_string()), until);
    }

    /// Whether `(provider, model)` is still within its cooldown window. An
    /// expired entry reads as not-cooling (and is overwritten on the next
    /// [`cool`](Self::cool)).
    pub(crate) async fn is_cooling(&self, provider: &str, model: &str) -> bool {
        let key = (provider.to_string(), model.to_string());
        self.0
            .read()
            .await
            .get(&key)
            .is_some_and(|&until| until > Instant::now())
    }

    /// Snapshot every `(provider, model)` pair still inside its cooldown
    /// window as `(provider, model, remaining_secs)`, sorted. Expired entries
    /// are omitted — same strict `until > now` reading as
    /// `is_cooling`. Diagnostic only — feeds health recovery and inspection.
    pub async fn snapshot(&self) -> Vec<(String, String, u64)> {
        let now = Instant::now();
        let mut out: Vec<(String, String, u64)> = self
            .0
            .read()
            .await
            .iter()
            .filter(|&((_, _), &until)| until > now)
            // rust-doctor-disable-next-line excessive-clone
            .map(|((p, m), &until)| (p.clone(), m.clone(), (until - now).as_secs()))
            .collect();
        out.sort();
        out
    }
}

/// Per-provider rate-limit cooldown gate (proactive pacing).
///
/// After a provider 429s / overloads and the walk gives up its in-place retries,
/// the provider name is parked until `available_at`. Before re-dialing that
/// provider on a *later* turn, the walk waits out the **remaining** window
/// instead of immediately re-triggering the same throttle — so a single paid
/// primary (e.g. Kimi) is paced to its rate limit and kept in use, rather than
/// eating a fresh 429 and bouncing to a fallback every turn. Provider-keyed twin
/// of [`ModelCooldown`]; it *waits* (the primary has no equivalent sibling)
/// where the model cooldown *sidelines*. Mirrors hermes'
/// `nous_rate_limit_remaining()` pre-request wait and opensquilla's credential
/// park. Cloning shares the same map (`Arc`), scoped exactly like
/// [`FailoverHealth`].
#[derive(Clone)]
pub struct ProviderCooldown(Arc<RwLock<HashMap<String, Instant>>>);

impl Default for ProviderCooldown {
    fn default() -> Self {
        Self(Arc::new(RwLock::new(HashMap::new())))
    }
}

impl ProviderCooldown {
    /// Park `provider` for `dur`. Extends an existing window, never shortens it,
    /// so a longer server `Retry-After` is not clobbered by a later default.
    pub(crate) async fn cool(&self, provider: &str, dur: Duration) {
        let until = Instant::now() + dur;
        let mut map = self.0.write().await;
        let slot = map.entry(provider.to_string()).or_insert(until);
        *slot = (*slot).max(until);
    }

    /// Drop `provider`'s pacing window, returning whether one was parked.
    ///
    /// Called when a request to `provider` **succeeds**. The window is recorded
    /// from a 429, and a *model-scoped* 429 records it too — so a provider whose
    /// model A is throttled gets parked whole, and the sibling-model migration
    /// that then answers on model B leaves the park in place. The next turn
    /// would defer that provider to a later candidate (or, as the last one, sit
    /// out the window) although it just served a request.
    ///
    /// A completed call is stronger evidence than the connectivity probe that
    /// [`FailoverHealth::reset`] deliberately refuses to clear this for: the
    /// probe only proves the endpoint answers, whereas this consumed the very
    /// rate budget the window was protecting.
    pub(crate) async fn clear(&self, provider: &str) -> bool {
        self.0.write().await.remove(provider).is_some()
    }

    /// Remaining cooldown for `provider`, or `None` if it is not currently
    /// cooling (no entry, or the window already elapsed).
    pub(crate) async fn remaining(&self, provider: &str) -> Option<Duration> {
        let now = Instant::now();
        self.0
            .read()
            .await
            .get(provider)
            .and_then(|&until| until.checked_duration_since(now))
    }

    /// Snapshot every provider still inside its pacing window as
    /// `(provider, remaining_secs)`, name-sorted. Expired entries are omitted
    /// — same strict `until > now` reading as `remaining`.
    /// Diagnostic only — feeds health recovery and inspection.
    pub async fn snapshot(&self) -> Vec<(String, u64)> {
        let now = Instant::now();
        let mut out: Vec<(String, u64)> = self
            .0
            .read()
            .await
            .iter()
            .filter(|&(_, &until)| until > now)
            // rust-doctor-disable-next-line excessive-clone
            .map(|(p, &until)| (p.clone(), (until - now).as_secs()))
            .collect();
        out.sort();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The variant is the severity of this handle going missing, DERIVED from
    /// the consumers of `global_health()`. Pinned in the module that owns the
    /// handle, because that is the only place a reclassification and a re-read
    /// of those consumers can be made to happen together.
    /// `census::every_slot_pins_its_own_missing_semantics` requires this by
    /// slot id.
    #[test]
    fn the_failover_health_slot_pins_its_missing_semantics() {
        assert_eq!(failover_health_slot().id(), "providers/failover-health");
        assert!(
            matches!(
                failover_health_slot().missing(),
                MissingSemantics::ConsumerDecides
            ),
            "`providers/failover-health` is classified ConsumerDecides from its consumers; \
             changing that means re-reading them, not re-typing this line"
        );
    }
}
