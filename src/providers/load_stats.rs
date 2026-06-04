//! Lock-free per-provider runtime load statistics for the failover balancer.
//!
//! The [`route_policy`](crate::providers::route_policy) load-balancing
//! strategies ([`LoadBalanceStrategy`](crate::config::types::LoadBalanceStrategy))
//! need two prompt-blind hard signals about each candidate provider: how many
//! requests are *in flight* right now, and how *fast* it has been responding.
//! This module owns both, keyed by provider name in a [`DashMap`] so the
//! failover hot path reads them with no global lock (the Rust edge over the
//! reference routers' mutex/Redis-guarded Python state).
//!
//! The map is shared (behind an `Arc`) across the global chain and every
//! per-agent override chain — exactly like [`FailoverHealth`] — so one
//! endpoint's load is visible to every chain that might dial it.
//!
//! R7/R10 stance: this is **infrastructure** (赋能层). It records counts and
//! latencies; it never sees the prompt, never classifies intent. The harness
//! loop is unaware it exists.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;

use crate::providers::route_policy::LoadMetric;

/// EWMA smoothing: `new = old*(1 - 1/8) + sample/8`. A small alpha (1/8) keeps
/// the latency estimate stable against single slow samples while still tracking
/// a sustained shift within a handful of requests.
const EWMA_SHIFT: u32 = 3; // divide by 2^3 = 8

/// Live load counters for one provider. Both fields are plain relaxed atomics —
/// load balancing is advisory ordering, not a synchronisation point, so a reader
/// racing a writer simply sees a value one request stale, which is exactly the
/// "next request picks it up" contract the route handle already documents.
#[derive(Debug, Default)]
pub struct ProviderLoad {
    /// Requests currently being awaited against this provider.
    in_flight: AtomicUsize,
    /// Exponentially-weighted moving average of observed latency, in
    /// microseconds. `0` means "no sample yet" — treated as best (tried first)
    /// by [`LoadBalanceStrategy::LatencyAware`] so a fresh endpoint gets
    /// measured rather than starved.
    latency_ewma_us: AtomicU64,
}

impl ProviderLoad {
    /// Fold one observed round-trip into the EWMA. Saturates to `u64::MAX`
    /// microseconds for absurdly long samples (no overflow, no panic).
    fn record_latency(&self, dur: Duration) {
        let sample = u64::try_from(dur.as_micros()).unwrap_or(u64::MAX);
        let old = self.latency_ewma_us.load(Ordering::Relaxed);
        let next = if old == 0 {
            sample
        } else {
            // old - old/8 + sample/8  ==  old*7/8 + sample/8, overflow-free.
            old - (old >> EWMA_SHIFT) + (sample >> EWMA_SHIFT)
        };
        self.latency_ewma_us.store(next, Ordering::Relaxed);
    }

    /// Snapshot this provider's metric for ordering decisions.
    fn metric(&self) -> LoadMetric {
        LoadMetric {
            in_flight: self.in_flight.load(Ordering::Relaxed),
            latency_us: self.latency_ewma_us.load(Ordering::Relaxed),
        }
    }
}

/// Process-wide runtime load registry, keyed by provider name.
///
/// Cheap to clone when wrapped in an `Arc`; the `DashMap` is the shared state.
#[derive(Debug, Default)]
pub struct LoadStats {
    providers: DashMap<String, Arc<ProviderLoad>>,
    /// Monotonic counter driving [`LoadBalanceStrategy::RoundRobin`] rotation.
    round_robin: AtomicU64,
}

impl LoadStats {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get-or-create the load handle for `name`. First touch inserts a default
    /// (all-zero) entry; later touches return the shared `Arc`.
    fn handle(&self, name: &str) -> Arc<ProviderLoad> {
        if let Some(load) = self.providers.get(name) {
            return load.clone();
        }
        self.providers.entry(name.to_string()).or_default().clone()
    }

    /// Mark one request as starting against `name`, returning an RAII guard that
    /// decrements the in-flight count when dropped — on success, error, *and*
    /// panic (P7 defensive: the count can never leak). Record the round-trip
    /// latency on the guard after a successful call.
    pub fn begin(&self, name: &str) -> InFlightGuard {
        let load = self.handle(name);
        load.in_flight.fetch_add(1, Ordering::Relaxed);
        InFlightGuard { load }
    }

    /// Snapshot the current metric for `name` (zero/default if never seen).
    pub fn metric(&self, name: &str) -> LoadMetric {
        self.providers
            .get(name)
            .map(|l| l.metric())
            .unwrap_or_default()
    }

    /// Next rotation index for round-robin selection. One increment per request.
    pub fn next_round_robin(&self) -> u64 {
        self.round_robin.fetch_add(1, Ordering::Relaxed)
    }
}

/// RAII in-flight counter. Increments on [`LoadStats::begin`], decrements on
/// drop. Holds a cheap `Arc` to the provider's counters so latency can be
/// recorded without a second map lookup.
#[derive(Debug)]
pub struct InFlightGuard {
    load: Arc<ProviderLoad>,
}

impl InFlightGuard {
    /// Fold a successful call's latency into this provider's EWMA.
    pub fn record_latency(&self, dur: Duration) {
        self.load.record_latency(dur);
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.load.in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_flight_increments_and_drops_back_to_zero() {
        let stats = LoadStats::new();
        assert_eq!(stats.metric("p").in_flight, 0);
        {
            let _g1 = stats.begin("p");
            let _g2 = stats.begin("p");
            assert_eq!(stats.metric("p").in_flight, 2);
        }
        // Both guards dropped → count back to zero.
        assert_eq!(stats.metric("p").in_flight, 0);
    }

    #[test]
    fn unknown_provider_metric_is_default() {
        let stats = LoadStats::new();
        let m = stats.metric("never-seen");
        assert_eq!(m.in_flight, 0);
        assert_eq!(m.latency_us, 0);
    }

    #[test]
    fn first_latency_sample_seeds_ewma_then_smooths() {
        let stats = LoadStats::new();
        let g = stats.begin("p");
        g.record_latency(Duration::from_micros(800));
        // First sample seeds the EWMA exactly.
        assert_eq!(stats.metric("p").latency_us, 800);
        // A higher sample nudges the average up but is damped (alpha = 1/8).
        g.record_latency(Duration::from_micros(1600));
        let v = stats.metric("p").latency_us;
        // 800 - 100 + 200 = 900
        assert_eq!(v, 900);
        assert!(v < 1600, "EWMA must damp a single high sample");
    }

    #[test]
    fn round_robin_counter_advances_monotonically() {
        let stats = LoadStats::new();
        assert_eq!(stats.next_round_robin(), 0);
        assert_eq!(stats.next_round_robin(), 1);
        assert_eq!(stats.next_round_robin(), 2);
    }

    #[test]
    fn latency_record_saturates_without_panic() {
        let stats = LoadStats::new();
        let g = stats.begin("p");
        // An absurd duration must saturate, not overflow/panic.
        g.record_latency(Duration::from_secs(u64::MAX / 1000));
        assert!(stats.metric("p").latency_us > 0);
    }
}
