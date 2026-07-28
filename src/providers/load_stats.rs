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
//! R7/R10 stance: this is **infrastructure** (infrastructure layer). It records counts and
//! latencies; it never sees the prompt, never classifies intent. The harness
//! loop is unaware it exists.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;

use crate::providers::route_policy::LoadMetric;

/// EWMA smoothing: `new = old*(1 - 1/8) + sample/8`. A small alpha (1/8) keeps
/// the latency estimate stable against single slow samples while still tracking
/// a sustained shift within a handful of requests.
const EWMA_SHIFT: u32 = 3; // divide by 2^3 = 8

/// Current rate-window index as a *monotonic* minute count since process start.
///
/// Deliberately monotonic (a process-global [`Instant`] base) rather than wall
/// clock: an NTP step or DST jump can only ever cost one window's worth of
/// counts, never a negative duration or a permanently-stuck window. `LiteLLM`'s
/// wall-clock minute buckets do not have this guarantee.
fn now_min() -> u64 {
    static BASE: OnceLock<Instant> = OnceLock::new();
    BASE.get_or_init(Instant::now).elapsed().as_secs() / 60
}

/// One provider's rolling 60s usage window: request and token counts that reset
/// when the minute rolls over. Lock-free — a roll is a single CAS on the epoch.
#[derive(Debug, Default)]
struct RateWindow {
    /// Monotonic-minute index this window's counters belong to.
    epoch_min: AtomicU64,
    /// Requests started in the current window.
    req: AtomicU32,
    /// Tokens (input + output) consumed in the current window.
    tokens: AtomicU64,
}

impl RateWindow {
    /// Roll the window if the minute changed: the thread that wins the epoch CAS
    /// zeroes the counters; losers proceed against the freshly-reset (or being-
    /// reset) window. A reader/writer racing the reset sees a value at most one
    /// request stale — the same advisory contract the rest of this module keeps.
    fn roll(&self, now: u64) {
        let cur = self.epoch_min.load(Ordering::Relaxed);
        if cur != now
            && self
                .epoch_min
                .compare_exchange(cur, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            self.req.store(0, Ordering::Relaxed);
            self.tokens.store(0, Ordering::Relaxed);
        }
    }

    /// Count one request against the current window.
    fn bump_req(&self) {
        self.roll(now_min());
        self.req.fetch_add(1, Ordering::Relaxed);
    }

    /// Add a successful call's tokens to the current window.
    fn add_tokens(&self, n: u64) {
        self.roll(now_min());
        self.tokens.fetch_add(n, Ordering::Relaxed);
    }

    /// Snapshot `(req, tokens)` for the live window (rolling first so a provider
    /// that went quiet does not report stale full counts).
    fn snapshot(&self) -> (u32, u64) {
        self.roll(now_min());
        (
            self.req.load(Ordering::Relaxed),
            self.tokens.load(Ordering::Relaxed),
        )
    }
}

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
    /// Rolling 60s request/token window driving
    /// [`LoadBalanceStrategy::UsageBased`] and the over-limit gate.
    window: RateWindow,
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

    /// Add a successful call's tokens to the rolling usage window.
    fn record_tokens(&self, n: u64) {
        self.window.add_tokens(n);
    }

    /// Snapshot this provider's metric for ordering decisions. Carries the raw
    /// window counts (`rpm_used`/`tpm_used`); the failover layer folds them
    /// against the configured limit into `utilization_permille`/`over_limit`.
    fn metric(&self) -> LoadMetric {
        let (rpm_used, tpm_used) = self.window.snapshot();
        LoadMetric {
            in_flight: self.in_flight.load(Ordering::Relaxed),
            latency_us: self.latency_ewma_us.load(Ordering::Relaxed),
            rpm_used,
            tpm_used,
            ..Default::default()
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
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Get-or-create the load handle for `name`. First touch inserts a default
    /// (all-zero) entry; later touches return the shared `Arc`.
    fn handle(&self, name: &str) -> Arc<ProviderLoad> {
        if let Some(load) = self.providers.get(name) {
            // rust-doctor-disable-next-line excessive-clone
            return load.clone();
        }
        // rust-doctor-disable-next-line excessive-clone
        self.providers.entry(name.to_string()).or_default().clone()
    }

    /// Mark one request as starting against `name`, returning an RAII guard that
    /// decrements the in-flight count when dropped — on success, error, *and*
    /// panic (P7 defensive: the count can never leak). Record the round-trip
    /// latency on the guard after a successful call.
    pub fn begin(&self, name: &str) -> InFlightGuard {
        let load = self.handle(name);
        load.in_flight.fetch_add(1, Ordering::Relaxed);
        // Count the request against the rolling rate window (RPM). Already on the
        // hot path, so usage tracking adds no new call site.
        load.window.bump_req();
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

    /// The rotation index the *next* request will use, without consuming it.
    ///
    /// For the read-only order preview in `route_status`: a diagnostic that
    /// advanced the counter would rotate the very ordering it claims to be
    /// reporting, so looking must not be a side effect.
    pub fn peek_round_robin(&self) -> u64 {
        self.round_robin.load(Ordering::Relaxed)
    }

    /// Snapshot every tracked provider's metric as `(name, metric)`,
    /// name-sorted. Raw window counts only — the caller folds configured
    /// limits in, same contract as [`metric`](Self::metric). Diagnostic only —
    /// feeds the `route_status` snapshot.
    pub fn all_metrics(&self) -> Vec<(String, LoadMetric)> {
        let mut out: Vec<(String, LoadMetric)> = self
            .providers
            .iter()
            // rust-doctor-disable-next-line excessive-clone
            .map(|e| (e.key().clone(), e.value().metric()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
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

    /// Add a successful call's tokens (input + output) to this provider's
    /// rolling usage window — drives TPM-based ordering and the over-limit gate.
    pub fn record_tokens(&self, n: u64) {
        self.load.record_tokens(n);
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

    #[test]
    fn begin_counts_requests_in_the_window() {
        let stats = LoadStats::new();
        assert_eq!(stats.metric("p").rpm_used, 0);
        let _g1 = stats.begin("p");
        let _g2 = stats.begin("p");
        // RPM accrues even while in-flight; the window count is independent of
        // the in-flight gauge (which drops on guard Drop).
        assert_eq!(stats.metric("p").rpm_used, 2);
    }

    #[test]
    fn record_tokens_accrues_in_the_window() {
        let stats = LoadStats::new();
        let g = stats.begin("p");
        g.record_tokens(120);
        g.record_tokens(80);
        assert_eq!(stats.metric("p").tpm_used, 200);
    }

    #[test]
    fn window_rolls_and_resets_counts() {
        // Drive the RateWindow directly across a minute boundary.
        let w = RateWindow::default();
        w.roll(10);
        w.bump_req();
        w.add_tokens(500);
        assert_eq!(w.snapshot(), (1, 500));
        // A later minute rolls the window → counters reset.
        w.roll(11);
        assert_eq!(w.snapshot(), (0, 0));
    }

    #[test]
    fn all_metrics_lists_tracked_providers_name_sorted() {
        let stats = LoadStats::new();
        assert!(stats.all_metrics().is_empty());
        let _gz = stats.begin("zeta");
        let _ga = stats.begin("alpha");
        let snap = stats.all_metrics();
        let names: Vec<&str> = snap.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["alpha", "zeta"]);
        assert_eq!(snap[0].1.in_flight, 1);
    }

    #[test]
    fn fresh_metric_carries_zero_usage_fields() {
        let stats = LoadStats::new();
        let m = stats.metric("never-seen");
        assert_eq!(m.rpm_used, 0);
        assert_eq!(m.tpm_used, 0);
        // Derived fields are filled by the failover layer, not load_stats.
        assert_eq!(m.utilization_permille, 0);
        assert!(!m.over_limit);
    }
}
