//! Tool health probe surface.
//!
//! Hermes-inspired `check_fn` TTL gate. Each tool may register a probe
//! that reports whether its runtime dependencies are alive (Docker
//! daemon, auth token, upstream MCP server, depth budget, etc.). The
//! [`ToolCatalog`] consults the cache alongside the
//! existing `is_active` flag when emitting native tool schemas via
//! `generate_smart_prompt`, so the LLM never sees a tool it cannot use.
//!
//! Design notes:
//!
//! * `ToolHealthCache` is the cross-registry rendezvous point. Probes
//!   are registered by tool **name** (not via `ToolHandler`) so the
//!   `tool_metadata` catalog and the executor's handler registry
//!   stay decoupled.
//! * Probes run with a 200 ms hard deadline (`PROBE_DEADLINE`) under a
//!   single-flight `tokio::sync::OnceCell` so concurrent triggers
//!   coalesce.
//! * Successful results are TTL-cached (default 30 s). Expired entries
//!   are reported as "healthy" to the snapshot — the LLM still sees the
//!   tool for one more turn while a background refresh re-evaluates.
//!   This matches hermes's "favour availability over latency" stance.
//! * `invalidate_all()` clears the cache and bumps `generation`; the
//!   catalog's own mutation methods (`register_with_conflict_resolution`,
//!   `remove_by_mcp_server`, …) call it directly after changing the tool set.

use crate::sync_primitives::Arc;
use crate::sync_primitives::{AtomicU64, Mutex, MutexGuard, Ordering};
use std::borrow::Cow;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;
use tokio::time::timeout;

/// Hard deadline for a single probe invocation. Probes that exceed this
/// are recorded as `Unhealthy { DependencyDown("probe timeout") }`.
const PROBE_DEADLINE: Duration = Duration::from_millis(200);

/// Default TTL between probe re-evaluations for a single tool.
pub const DEFAULT_PROBE_TTL: Duration = Duration::from_secs(30);

/// Why a tool is currently unhealthy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum HealthReason {
    DependencyDown(Cow<'static, str>),
    AuthMissing(Cow<'static, str>),
    /// Wall-clock millis-from-epoch when the rate-limit lifts.
    RateLimited {
        until_ms_from_epoch: i64,
    },
    Custom(Cow<'static, str>),
}

impl HealthReason {
    /// One-line label suitable for prompt or log output.
    #[must_use]
    pub fn short_label(&self) -> &str {
        match self {
            Self::DependencyDown(s) | Self::AuthMissing(s) | Self::Custom(s) => s,
            Self::RateLimited { .. } => "rate limited",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum ProbeResult {
    Healthy,
    Unhealthy {
        reason: HealthReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after: Option<Duration>,
    },
}

/// A tool opts into health gating by implementing this trait and
/// registering a probe with [`ToolHealthCache::register_probe`].
#[async_trait::async_trait]
pub trait ToolHealthProbe: Send + Sync {
    /// Cheap availability check. Implementations MUST be bounded — the
    /// cache enforces a 200 ms hard deadline.
    async fn probe(&self) -> ProbeResult;

    /// How long a successful probe result may be cached.
    fn ttl(&self) -> Duration {
        DEFAULT_PROBE_TTL
    }
}

#[derive(Clone)]
struct CachedProbe {
    result: ProbeResult,
    cached_at: Instant,
    ttl: Duration,
}

impl CachedProbe {
    fn expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.cached_at) >= self.ttl
    }
}

/// Every registration receives a fresh identity, including registration of
/// the same probe implementation twice.
struct ProbeRegistration {
    probe: Arc<dyn ToolHealthProbe>,
}

struct Flight {
    cell: Arc<OnceCell<ProbeResult>>,
    registration: Arc<ProbeRegistration>,
    participants: usize,
    published: bool,
}

#[derive(Default)]
struct WriterState {
    flights: HashMap<String, Flight>,
}

/// In-process TTL cache of probe results. Reads are lock-free via
/// [`ArcSwap`]. All writes and flight identity transitions use one short,
/// poison-recovering synchronous writer mutex; probes and TTL callbacks never
/// run while that mutex is held.
pub struct ToolHealthCache {
    entries: ArcSwap<HashMap<String, CachedProbe>>,
    probes: ArcSwap<HashMap<String, Arc<ProbeRegistration>>>,
    writer: Mutex<WriterState>,
    generation: AtomicU64,
    #[cfg(test)]
    participant_notify: tokio::sync::Notify,
}

impl ToolHealthCache {
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: ArcSwap::from_pointee(HashMap::new()),
            probes: ArcSwap::from_pointee(HashMap::new()),
            writer: Mutex::new(WriterState::default()),
            generation: AtomicU64::new(0),
            #[cfg(test)]
            participant_notify: tokio::sync::Notify::new(),
        }
    }

    fn lock_writer(&self) -> MutexGuard<'_, WriterState> {
        match self.writer.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn clear_entry_locked(&self, name: &str) {
        self.entries.rcu(|entries| {
            if !entries.contains_key(name) {
                return entries.clone();
            }
            let mut next = (**entries).clone();
            next.remove(name);
            Arc::new(next)
        });
    }

    /// Register a probe under `name`. Replaces any prior probe for the
    /// same name. Bumps generation so downstream caches (e.g.
    /// `ToolEmitCache`) invalidate on the next read.
    pub fn register_probe(&self, name: impl Into<String>, probe: Arc<dyn ToolHealthProbe>) {
        let name = name.into();
        let registration = Arc::new(ProbeRegistration { probe });
        let (retired_probes, retired_flight) = {
            let mut writer = self.lock_writer();
            let current = self.probes.load_full();
            let mut next = (*current).clone();
            next.insert(name.clone(), registration);
            let retired_probes = self.probes.swap(Arc::new(next));
            self.clear_entry_locked(&name);
            let retired_flight = writer.flights.remove(&name);
            self.generation.fetch_add(1, Ordering::Release);
            (retired_probes, retired_flight)
        };
        drop(retired_flight);
        drop(retired_probes);
    }

    /// Remove a probe by name. Bumps generation. Returns `true` if a
    /// probe was actually removed. Entries and flights are cleared even
    /// when stale residue remains after an earlier replacement.
    pub fn unregister_probe(&self, name: &str) -> bool {
        let (removed, retired_probes, retired_flight) = {
            let mut writer = self.lock_writer();
            let current = self.probes.load_full();
            let mut next = (*current).clone();
            let removed = next.remove(name).is_some();
            let retired_probes = self.probes.swap(Arc::new(next));
            self.clear_entry_locked(name);
            let retired_flight = writer.flights.remove(name);
            if removed {
                self.generation.fetch_add(1, Ordering::Release);
            }
            (removed, retired_probes, retired_flight)
        };
        drop(retired_flight);
        drop(retired_probes);
        removed
    }

    /// Lock-free read snapshot. Tools without a probe report healthy.
    pub fn snapshot(&self) -> HealthSnapshot {
        HealthSnapshot {
            entries: self.entries.load_full(),
        }
    }

    /// Names of every currently-registered probe. Used by the discovery
    /// registry to drive background refreshes off the probe set itself
    /// (rather than a catalog tool list), so a probe keyed by an
    /// executor-only name still gets evaluated.
    pub fn probe_names(&self) -> Vec<String> {
        self.probes.load().keys().cloned().collect()
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Clear every cached entry and bump generation. Called when the
    /// underlying tool registry mutates (broadcast subscriber).
    /// Does NOT clear registered probes.
    pub fn invalidate_all(&self) {
        let retired_flights = {
            let mut writer = self.lock_writer();
            self.entries.store(Arc::new(HashMap::new()));
            let retired_flights = writer
                .flights
                .drain()
                .map(|(_, flight)| flight)
                .collect::<Vec<_>>();
            self.generation.fetch_add(1, Ordering::Release);
            retired_flights
        };
        drop(retired_flights);
    }

    /// Returns true if `name` has a registered probe AND either has no
    /// cache entry or the entry has expired.
    pub fn needs_refresh(&self, name: &str) -> bool {
        if !self.probes.load().contains_key(name) {
            return false;
        }
        let entries = self.entries.load();
        match entries.get(name) {
            None => true,
            Some(entry) => entry.expired(Instant::now()),
        }
    }

    fn current_result_or_healthy(&self, name: &str) -> ProbeResult {
        let now = Instant::now();
        self.entries
            .load()
            .get(name)
            .filter(|entry| !entry.expired(now))
            .map(|entry| entry.result.clone())
            .unwrap_or(ProbeResult::Healthy)
    }

    /// Run the probe with timeout + single-flight, store the result,
    /// bump generation. Returns the freshly-stored result. Does nothing
    /// (returns `Healthy`) if `name` has no registered probe.
    pub async fn refresh(&self, name: &str) -> ProbeResult {
        let (registration, cell, retired_flight) = {
            let mut writer = self.lock_writer();
            let registration = match self.probes.load().get(name).cloned() {
                Some(registration) => registration,
                None => return ProbeResult::Healthy,
            };
            let mut retired_flight = None;
            let cell = match writer.flights.get_mut(name) {
                Some(flight)
                    if Arc::ptr_eq(&flight.registration, &registration) && !flight.published =>
                {
                    flight.participants += 1;
                    flight.cell.clone()
                }
                _ => {
                    let cell = Arc::new(OnceCell::new());
                    retired_flight = writer.flights.insert(
                        name.to_string(),
                        Flight {
                            cell: cell.clone(),
                            registration: registration.clone(),
                            participants: 1,
                            published: false,
                        },
                    );
                    cell
                }
            };
            #[cfg(test)]
            self.participant_notify.notify_waiters();
            (registration, cell, retired_flight)
        };
        drop(retired_flight);

        struct ParticipantGuard<'a> {
            cache: &'a ToolHealthCache,
            name: String,
            cell: Arc<OnceCell<ProbeResult>>,
        }

        impl Drop for ParticipantGuard<'_> {
            fn drop(&mut self) {
                let retired = {
                    let mut writer = self.cache.lock_writer();
                    let remove = match writer.flights.get_mut(&self.name) {
                        Some(flight) if Arc::ptr_eq(&flight.cell, &self.cell) => {
                            debug_assert!(flight.participants > 0);
                            flight.participants -= 1;
                            flight.participants == 0
                        }
                        _ => false,
                    };
                    if remove {
                        writer.flights.remove(&self.name)
                    } else {
                        None
                    }
                };
                drop(retired);
            }
        }

        let _participant = ParticipantGuard {
            cache: self,
            name: name.to_string(),
            cell: cell.clone(),
        };
        let result = cell
            .get_or_init(|| async {
                let bounded = timeout(PROBE_DEADLINE, registration.probe.probe()).await;
                bounded.unwrap_or(ProbeResult::Unhealthy {
                    reason: HealthReason::DependencyDown(Cow::Borrowed("probe timeout")),
                    retry_after: None,
                })
            })
            .await
            .clone();
        let ttl = registration.probe.ttl();

        let mut writer = self.lock_writer();
        let current_registration = self.probes.load().get(name).cloned();
        let current = writer.flights.get_mut(name);
        let is_current = current_registration
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &registration))
            && current
                .as_ref()
                .is_some_and(|flight| Arc::ptr_eq(&flight.cell, &cell));
        if !is_current {
            drop(writer);
            return self.current_result_or_healthy(name);
        }

        let flight = current.expect("current flight was checked above");
        if !flight.published {
            let cached = CachedProbe {
                result: result.clone(),
                cached_at: Instant::now(),
                ttl,
            };
            self.entries.rcu(|entries| {
                let mut next = (**entries).clone();
                next.insert(name.to_string(), cached.clone());
                Arc::new(next)
            });
            self.generation.fetch_add(1, Ordering::Release);
            flight.published = true;
        }
        result
    }
}

impl Default for ToolHealthCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Immutable snapshot of the cache state.
#[derive(Clone, Default)]
pub struct HealthSnapshot {
    entries: Arc<HashMap<String, CachedProbe>>,
}

impl HealthSnapshot {
    /// Tools without a cached entry report healthy (no probe, or the
    /// probe is in flight). Expired entries also report healthy — the
    /// LLM still sees the tool while a background refresh re-evaluates.
    #[must_use]
    pub fn is_healthy(&self, name: &str) -> bool {
        let now = Instant::now();
        match self.entries.get(name) {
            None => true,
            Some(entry) if entry.expired(now) => true,
            Some(entry) => matches!(entry.result, ProbeResult::Healthy),
        }
    }

    /// If the tool is cached AS unhealthy and the entry hasn't expired,
    /// return the reason. Otherwise `None`.
    #[must_use]
    pub fn reason(&self, name: &str) -> Option<&HealthReason> {
        let now = Instant::now();
        match self.entries.get(name) {
            Some(entry) if !entry.expired(now) => match &entry.result {
                ProbeResult::Unhealthy { reason, .. } => Some(reason),
                ProbeResult::Healthy => None,
            },
            _ => None,
        }
    }

    /// Iterator over `(name, reason)` for every cached-unhealthy tool.
    pub fn unhealthy_iter(&self) -> impl Iterator<Item = (&str, &HealthReason)> + '_ {
        let now = Instant::now();
        self.entries.iter().filter_map(move |(name, entry)| {
            if entry.expired(now) {
                return None;
            }
            match &entry.result {
                ProbeResult::Unhealthy { reason, .. } => Some((name.as_str(), reason)),
                ProbeResult::Healthy => None,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    #[test]
    fn snapshot_without_entry_is_healthy() {
        let cache = ToolHealthCache::new();
        let snap = cache.snapshot();
        assert!(snap.is_healthy("any_tool"));
        assert!(snap.reason("any_tool").is_none());
    }

    #[test]
    fn health_reason_short_label() {
        assert_eq!(
            HealthReason::DependencyDown(Cow::Borrowed("docker daemon offline")).short_label(),
            "docker daemon offline"
        );
        assert_eq!(
            HealthReason::AuthMissing(Cow::Borrowed("TELEGRAM_BOT_TOKEN")).short_label(),
            "TELEGRAM_BOT_TOKEN"
        );
        assert_eq!(
            HealthReason::RateLimited {
                until_ms_from_epoch: 0
            }
            .short_label(),
            "rate limited"
        );
        assert_eq!(
            HealthReason::Custom(Cow::Borrowed("oops")).short_label(),
            "oops"
        );
    }

    #[test]
    fn probe_result_serialises_round_trip() {
        let r = ProbeResult::Unhealthy {
            reason: HealthReason::RateLimited {
                until_ms_from_epoch: 1_700_000_000_000,
            },
            retry_after: Some(Duration::from_secs(60)),
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: ProbeResult = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    struct AlwaysHealthy;
    #[async_trait::async_trait]
    impl ToolHealthProbe for AlwaysHealthy {
        async fn probe(&self) -> ProbeResult {
            ProbeResult::Healthy
        }
    }

    struct AlwaysDead;
    #[async_trait::async_trait]
    impl ToolHealthProbe for AlwaysDead {
        async fn probe(&self) -> ProbeResult {
            ProbeResult::Unhealthy {
                reason: HealthReason::DependencyDown(Cow::Borrowed("test fixture")),
                retry_after: None,
            }
        }
    }

    struct ScriptedProbe {
        starts: tokio::sync::Notify,
        calls: crate::sync_primitives::AtomicUsize,
        gates: Mutex<std::collections::VecDeque<tokio::sync::oneshot::Receiver<ProbeResult>>>,
        ttl: Duration,
    }

    impl ScriptedProbe {
        fn new(gates: Vec<tokio::sync::oneshot::Receiver<ProbeResult>>) -> Self {
            Self::with_ttl(gates, DEFAULT_PROBE_TTL)
        }

        fn with_ttl(
            gates: Vec<tokio::sync::oneshot::Receiver<ProbeResult>>,
            ttl: Duration,
        ) -> Self {
            Self {
                starts: tokio::sync::Notify::new(),
                calls: crate::sync_primitives::AtomicUsize::new(0),
                gates: Mutex::new(gates.into_iter().collect()),
                ttl,
            }
        }

        async fn wait_for_calls(&self, target: usize) {
            loop {
                let ready = self.starts.notified();
                if self.calls.load(Ordering::Acquire) >= target {
                    return;
                }
                ready.await;
            }
        }
    }

    #[async_trait::async_trait]
    impl ToolHealthProbe for ScriptedProbe {
        async fn probe(&self) -> ProbeResult {
            self.calls.fetch_add(1, Ordering::Release);
            self.starts.notify_waiters();
            let gate = match self.gates.lock() {
                Ok(mut gates) => gates.pop_front().expect("scripted probe gate"),
                Err(poisoned) => poisoned
                    .into_inner()
                    .pop_front()
                    .expect("scripted probe gate"),
            };
            gate.await.expect("scripted probe sender")
        }

        fn ttl(&self) -> Duration {
            self.ttl
        }
    }

    struct PanicOnceProbe {
        starts: tokio::sync::Notify,
        calls: crate::sync_primitives::AtomicUsize,
    }

    impl PanicOnceProbe {
        fn new() -> Self {
            Self {
                starts: tokio::sync::Notify::new(),
                calls: crate::sync_primitives::AtomicUsize::new(0),
            }
        }

        async fn wait_for_calls(&self, target: usize) {
            loop {
                let ready = self.starts.notified();
                if self.calls.load(Ordering::Acquire) >= target {
                    return;
                }
                ready.await;
            }
        }
    }

    #[async_trait::async_trait]
    impl ToolHealthProbe for PanicOnceProbe {
        async fn probe(&self) -> ProbeResult {
            let call = self.calls.fetch_add(1, Ordering::Release);
            self.starts.notify_waiters();
            if call == 0 {
                panic!("panic-once health probe");
            }
            ProbeResult::Healthy
        }
    }

    struct DropProbe {
        cache: std::sync::Weak<ToolHealthCache>,
        released: Arc<crate::sync_primitives::AtomicBool>,
    }

    impl Drop for DropProbe {
        fn drop(&mut self) {
            if let Some(cache) = self.cache.upgrade() {
                if cache.writer.try_lock().is_ok() {
                    self.released.store(true, Ordering::Release);
                    cache.invalidate_all();
                }
            }
        }
    }

    #[async_trait::async_trait]
    impl ToolHealthProbe for DropProbe {
        async fn probe(&self) -> ProbeResult {
            unhealthy("drop-probe")
        }
    }

    async fn wait_for_participants(cache: &ToolHealthCache, name: &str, target: usize) {
        loop {
            let ready = cache.participant_notify.notified();
            let participants = {
                let writer = cache.lock_writer();
                writer
                    .flights
                    .get(name)
                    .map_or(0, |flight| flight.participants)
            };
            if participants >= target {
                return;
            }
            ready.await;
        }
    }

    fn unhealthy(label: &'static str) -> ProbeResult {
        ProbeResult::Unhealthy {
            reason: HealthReason::DependencyDown(Cow::Borrowed(label)),
            retry_after: None,
        }
    }

    struct HangsForever;
    #[async_trait::async_trait]
    impl ToolHealthProbe for HangsForever {
        async fn probe(&self) -> ProbeResult {
            // Block past the 200ms deadline; the cache MUST give up
            // and record an Unhealthy("probe timeout") instead.
            tokio::time::sleep(Duration::from_secs(1)).await;
            ProbeResult::Healthy
        }
    }

    #[tokio::test]
    async fn refresh_with_no_probe_reports_healthy() {
        let cache = ToolHealthCache::new();
        let r = cache.refresh("missing").await;
        assert_eq!(r, ProbeResult::Healthy);
    }

    #[tokio::test]
    async fn refresh_dead_probe_caches_unhealthy() {
        let cache = ToolHealthCache::new();
        cache.register_probe("dead", Arc::new(AlwaysDead));
        let r = cache.refresh("dead").await;
        assert!(matches!(r, ProbeResult::Unhealthy { .. }));
        assert!(!cache.snapshot().is_healthy("dead"));
    }

    #[tokio::test]
    async fn refresh_healthy_probe_caches_healthy() {
        let cache = ToolHealthCache::new();
        cache.register_probe("alive", Arc::new(AlwaysHealthy));
        cache.refresh("alive").await;
        assert!(cache.snapshot().is_healthy("alive"));
    }

    #[tokio::test]
    async fn hanging_probe_is_treated_as_unhealthy_timeout() {
        let cache = ToolHealthCache::new();
        cache.register_probe("slow", Arc::new(HangsForever));
        let r = cache.refresh("slow").await;
        match r {
            ProbeResult::Unhealthy { reason, .. } => {
                assert!(reason.short_label().contains("timeout"));
            }
            _ => panic!("expected timeout-driven Unhealthy"),
        }
    }

    #[tokio::test]
    async fn invalidate_all_clears_entries_and_bumps_generation() {
        let cache = ToolHealthCache::new();
        cache.register_probe("x", Arc::new(AlwaysDead));
        cache.refresh("x").await;
        let g1 = cache.generation();
        assert!(!cache.snapshot().is_healthy("x"));
        cache.invalidate_all();
        // The entry is gone — snapshot reports healthy again.
        assert!(cache.snapshot().is_healthy("x"));
        assert!(cache.generation() > g1);
    }

    #[tokio::test]
    async fn needs_refresh_triggers_for_missing_and_expired() {
        let cache = ToolHealthCache::new();
        // No probe registered → never needs refresh.
        assert!(!cache.needs_refresh("ghost"));
        cache.register_probe("x", Arc::new(AlwaysHealthy));
        // Probe present but no entry yet → needs refresh.
        assert!(cache.needs_refresh("x"));
        cache.refresh("x").await;
        // Fresh entry → no refresh needed.
        assert!(!cache.needs_refresh("x"));
    }

    #[tokio::test]
    async fn unhealthy_iter_yields_only_cached_unhealthy() {
        let cache = ToolHealthCache::new();
        cache.register_probe("alive", Arc::new(AlwaysHealthy));
        cache.register_probe("dead", Arc::new(AlwaysDead));
        cache.refresh("alive").await;
        cache.refresh("dead").await;
        let snap = cache.snapshot();
        let unhealthy: Vec<_> = snap.unhealthy_iter().map(|(n, _)| n.to_string()).collect();
        assert_eq!(unhealthy, vec!["dead".to_string()]);
    }

    #[tokio::test]
    async fn unregister_probe_returns_true_when_present() {
        let cache = ToolHealthCache::new();
        cache.register_probe("x", Arc::new(AlwaysHealthy));
        assert!(cache.unregister_probe("x"));
        assert!(!cache.unregister_probe("x"));
    }

    #[tokio::test]
    async fn replacement_cannot_publish_obsolete_a_over_b() {
        let (a_tx, a_rx) = tokio::sync::oneshot::channel();
        let (b_tx, b_rx) = tokio::sync::oneshot::channel();
        let a_probe = Arc::new(ScriptedProbe::new(vec![a_rx]));
        let b_probe = Arc::new(ScriptedProbe::new(vec![b_rx]));
        let cache = Arc::new(ToolHealthCache::new());
        cache.register_probe("tool", a_probe.clone());
        let a_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        a_probe.wait_for_calls(1).await;

        cache.register_probe("tool", b_probe.clone());
        let generation_after_b = cache.generation();
        let b_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        b_probe.wait_for_calls(1).await;
        assert!(cache.snapshot().is_healthy("tool"));

        a_tx.send(unhealthy("a")).unwrap();
        assert_eq!(a_task.await.unwrap(), ProbeResult::Healthy);
        assert_eq!(cache.generation(), generation_after_b);
        assert!(cache.snapshot().is_healthy("tool"));

        b_tx.send(unhealthy("b")).unwrap();
        assert_eq!(b_task.await.unwrap(), unhealthy("b"));
        assert_eq!(cache.snapshot().reason("tool").unwrap().short_label(), "b");
        assert_eq!(cache.generation(), generation_after_b + 1);
    }

    #[tokio::test]
    async fn invalidation_keeps_new_same_registration_flight_alive() {
        let (a_tx, a_rx) = tokio::sync::oneshot::channel();
        let (b_tx, b_rx) = tokio::sync::oneshot::channel();
        let probe = Arc::new(ScriptedProbe::new(vec![a_rx, b_rx]));
        let cache = Arc::new(ToolHealthCache::new());
        cache.register_probe("tool", probe.clone());
        let a_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        probe.wait_for_calls(1).await;

        cache.invalidate_all();
        let b_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        probe.wait_for_calls(2).await;
        a_tx.send(unhealthy("a")).unwrap();
        assert_eq!(a_task.await.unwrap(), ProbeResult::Healthy);
        assert!(cache.snapshot().is_healthy("tool"));

        b_tx.send(unhealthy("b")).unwrap();
        assert_eq!(b_task.await.unwrap(), unhealthy("b"));
        assert_eq!(cache.snapshot().reason("tool").unwrap().short_label(), "b");
    }

    #[tokio::test]
    async fn reregistering_same_probe_arc_gets_fresh_registration_identity() {
        let (a_tx, a_rx) = tokio::sync::oneshot::channel();
        let (b_tx, b_rx) = tokio::sync::oneshot::channel();
        let probe = Arc::new(ScriptedProbe::new(vec![a_rx, b_rx]));
        let cache = Arc::new(ToolHealthCache::new());
        cache.register_probe("tool", probe.clone());
        let a_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        probe.wait_for_calls(1).await;

        assert!(cache.unregister_probe("tool"));
        assert!(cache.snapshot().is_healthy("tool"));
        cache.register_probe("tool", probe.clone());
        let b_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        probe.wait_for_calls(2).await;
        assert!(cache.snapshot().is_healthy("tool"));

        a_tx.send(unhealthy("a")).unwrap();
        assert_eq!(a_task.await.unwrap(), ProbeResult::Healthy);
        b_tx.send(unhealthy("b")).unwrap();
        assert_eq!(b_task.await.unwrap(), unhealthy("b"));
        assert_eq!(cache.snapshot().reason("tool").unwrap().short_label(), "b");
    }

    #[tokio::test]
    async fn obsolete_cancellation_cannot_remove_live_coalesced_flight() {
        let (a_tx, a_rx) = tokio::sync::oneshot::channel();
        let (b_tx, b_rx) = tokio::sync::oneshot::channel();
        let (next_tx, next_rx) = tokio::sync::oneshot::channel();
        let a_probe = Arc::new(ScriptedProbe::new(vec![a_rx]));
        let b_probe = Arc::new(ScriptedProbe::new(vec![b_rx, next_rx]));
        let cache = Arc::new(ToolHealthCache::new());
        cache.register_probe("tool", a_probe.clone());
        let a_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        a_probe.wait_for_calls(1).await;
        cache.register_probe("tool", b_probe.clone());

        let b1_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        b_probe.wait_for_calls(1).await;
        let b2_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        wait_for_participants(&cache, "tool", 2).await;
        a_task.abort();
        let _ = a_task.await;

        b_tx.send(unhealthy("b")).unwrap();
        assert_eq!(b1_task.await.unwrap(), unhealthy("b"));
        assert_eq!(b2_task.await.unwrap(), unhealthy("b"));
        assert_eq!(b_probe.calls.load(Ordering::Acquire), 1);
        assert_eq!(cache.writer.lock().unwrap().flights.len(), 0);

        let next_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        b_probe.wait_for_calls(2).await;
        next_tx.send(unhealthy("next")).unwrap();
        assert_eq!(next_task.await.unwrap(), unhealthy("next"));
        assert_eq!(b_probe.calls.load(Ordering::Acquire), 2);
        drop(a_tx);
    }

    #[tokio::test]
    async fn active_initializer_cancellation_retries_for_surviving_participant() {
        let (_first_tx, first_rx) = tokio::sync::oneshot::channel();
        let (second_tx, second_rx) = tokio::sync::oneshot::channel();
        let probe = Arc::new(ScriptedProbe::new(vec![first_rx, second_rx]));
        let cache = Arc::new(ToolHealthCache::new());
        cache.register_probe("tool", probe.clone());
        let generation_after_register = cache.generation();

        let first_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        probe.wait_for_calls(1).await;
        let second_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        wait_for_participants(&cache, "tool", 2).await;
        let cell_before_cancel = cache
            .writer
            .lock()
            .unwrap()
            .flights
            .get("tool")
            .unwrap()
            .cell
            .clone();

        first_task.abort();
        let _ = first_task.await;
        probe.wait_for_calls(2).await;
        let cell_after_retry = cache
            .writer
            .lock()
            .unwrap()
            .flights
            .get("tool")
            .unwrap()
            .cell
            .clone();
        assert!(Arc::ptr_eq(&cell_before_cancel, &cell_after_retry));
        let third_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        wait_for_participants(&cache, "tool", 2).await;
        assert!(Arc::ptr_eq(
            &cell_before_cancel,
            &cache.writer.lock().unwrap().flights["tool"].cell
        ));
        second_tx.send(unhealthy("retry")).unwrap();
        assert_eq!(second_task.await.unwrap(), unhealthy("retry"));
        assert_eq!(third_task.await.unwrap(), unhealthy("retry"));
        assert_eq!(probe.calls.load(Ordering::Acquire), 2);
        assert_eq!(cache.generation(), generation_after_register + 1);
        assert_eq!(
            cache.snapshot().reason("tool").unwrap().short_label(),
            "retry"
        );
        assert!(cache.writer.lock().unwrap().flights.is_empty());
    }

    #[tokio::test]
    async fn last_participant_cancellation_retires_slot_for_following_refresh() {
        let (_first_tx, first_rx) = tokio::sync::oneshot::channel();
        let (second_tx, second_rx) = tokio::sync::oneshot::channel();
        let probe = Arc::new(ScriptedProbe::new(vec![first_rx, second_rx]));
        let cache = Arc::new(ToolHealthCache::new());
        cache.register_probe("tool", probe.clone());

        let first_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        probe.wait_for_calls(1).await;
        first_task.abort();
        let _ = first_task.await;
        assert!(cache.writer.lock().unwrap().flights.is_empty());

        let second_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        probe.wait_for_calls(2).await;
        second_tx.send(ProbeResult::Healthy).unwrap();
        assert_eq!(second_task.await.unwrap(), ProbeResult::Healthy);
        assert_eq!(probe.calls.load(Ordering::Acquire), 2);
        assert!(cache.snapshot().is_healthy("tool"));
        assert!(cache.writer.lock().unwrap().flights.is_empty());
    }

    #[tokio::test]
    async fn last_participant_cancel_and_panic_recover_the_flight_slot() {
        let probe = Arc::new(PanicOnceProbe::new());
        let cache = Arc::new(ToolHealthCache::new());
        cache.register_probe("tool", probe.clone());
        let generation_after_register = cache.generation();

        let first_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        probe.wait_for_calls(1).await;
        let error = first_task.await.expect_err("panic-once probe must panic");
        assert!(error.is_panic());
        assert!(cache.writer.lock().unwrap().flights.is_empty());

        assert_eq!(cache.refresh("tool").await, ProbeResult::Healthy);
        assert_eq!(probe.calls.load(Ordering::Acquire), 2);
        assert!(cache.snapshot().is_healthy("tool"));
        assert_eq!(cache.generation(), generation_after_register + 1);
        assert!(cache.writer.lock().unwrap().flights.is_empty());
    }

    #[tokio::test]
    async fn direct_same_arc_reregistration_clears_cached_result() {
        let (first_tx, first_rx) = tokio::sync::oneshot::channel();
        let (second_tx, second_rx) = tokio::sync::oneshot::channel();
        let probe = Arc::new(ScriptedProbe::new(vec![first_rx, second_rx]));
        let cache = Arc::new(ToolHealthCache::new());
        cache.register_probe("tool", probe.clone());

        let first_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        probe.wait_for_calls(1).await;
        first_tx.send(unhealthy("old")).unwrap();
        assert_eq!(first_task.await.unwrap(), unhealthy("old"));
        let old_snapshot = cache.snapshot();
        assert_eq!(old_snapshot.reason("tool").unwrap().short_label(), "old");
        let generation_before_replace = cache.generation();

        cache.register_probe("tool", probe.clone());
        assert!(cache.snapshot().is_healthy("tool"));
        assert_eq!(old_snapshot.reason("tool").unwrap().short_label(), "old");
        assert_eq!(cache.generation(), generation_before_replace + 1);

        let second_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        probe.wait_for_calls(2).await;
        second_tx.send(unhealthy("new")).unwrap();
        assert_eq!(second_task.await.unwrap(), unhealthy("new"));
        assert_eq!(
            cache.snapshot().reason("tool").unwrap().short_label(),
            "new"
        );
    }

    #[tokio::test]
    async fn published_flight_with_unpolled_waiter_yields_fresh_zero_ttl_flight() {
        let (first_tx, first_rx) = tokio::sync::oneshot::channel();
        let (fresh_tx, fresh_rx) = tokio::sync::oneshot::channel();
        let probe = Arc::new(ScriptedProbe::with_ttl(
            vec![first_rx, fresh_rx],
            Duration::ZERO,
        ));
        let cache = Arc::new(ToolHealthCache::new());
        cache.register_probe("tool", probe.clone());
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);

        let mut first = Box::pin(cache.refresh("tool"));
        assert!(matches!(first.as_mut().poll(&mut context), Poll::Pending));
        let mut waiter = Box::pin(cache.refresh("tool"));
        assert!(matches!(waiter.as_mut().poll(&mut context), Poll::Pending));
        assert_eq!(cache.writer.lock().unwrap().flights["tool"].participants, 2);

        first_tx.send(ProbeResult::Healthy).unwrap();
        assert_eq!(
            first.as_mut().poll(&mut context),
            Poll::Ready(ProbeResult::Healthy)
        );
        drop(first);
        let generation_after_first = cache.generation();

        let fresh_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        probe.wait_for_calls(2).await;
        let fresh_cell = cache.writer.lock().unwrap().flights["tool"].cell.clone();
        assert_eq!(
            waiter.as_mut().poll(&mut context),
            Poll::Ready(ProbeResult::Healthy)
        );
        drop(waiter);
        assert!(Arc::ptr_eq(
            &fresh_cell,
            &cache.writer.lock().unwrap().flights["tool"].cell
        ));
        assert_eq!(cache.generation(), generation_after_first);

        fresh_tx.send(unhealthy("fresh")).unwrap();
        assert_eq!(fresh_task.await.unwrap(), unhealthy("fresh"));
        assert_eq!(cache.generation(), generation_after_first + 1);
        assert!(cache.snapshot().is_healthy("tool"));
        assert!(cache.writer.lock().unwrap().flights.is_empty());
    }

    #[tokio::test]
    async fn probe_drop_observes_released_writer_and_can_invalidate() {
        let cache = Arc::new(ToolHealthCache::new());
        let released = Arc::new(crate::sync_primitives::AtomicBool::new(false));
        cache.register_probe(
            "tool",
            Arc::new(DropProbe {
                cache: Arc::downgrade(&cache),
                released: released.clone(),
            }),
        );
        assert!(matches!(
            cache.refresh("tool").await,
            ProbeResult::Unhealthy { .. }
        ));
        assert!(!cache.snapshot().is_healthy("tool"));
        let generation_before_unregister = cache.generation();

        // The cache owns the last probe reference; unregister triggers Drop.
        assert!(cache.unregister_probe("tool"));
        assert!(released.load(Ordering::Acquire));
        assert_eq!(cache.generation(), generation_before_unregister + 2);
        assert!(cache.snapshot().is_healthy("tool"));
    }

    #[tokio::test]
    async fn zero_ttl_refresh_starts_a_new_flight_after_publication() {
        let (first_tx, first_rx) = tokio::sync::oneshot::channel();
        let (second_tx, second_rx) = tokio::sync::oneshot::channel();
        let probe = Arc::new(ScriptedProbe::with_ttl(
            vec![first_rx, second_rx],
            Duration::ZERO,
        ));
        let cache = Arc::new(ToolHealthCache::new());
        cache.register_probe("tool", probe.clone());

        let first_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        probe.wait_for_calls(1).await;
        first_tx.send(ProbeResult::Healthy).unwrap();
        assert_eq!(first_task.await.unwrap(), ProbeResult::Healthy);

        let second_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        probe.wait_for_calls(2).await;
        second_tx.send(unhealthy("second")).unwrap();
        assert_eq!(second_task.await.unwrap(), unhealthy("second"));
        assert_eq!(probe.calls.load(Ordering::Acquire), 2);
    }

    #[tokio::test]
    async fn obsolete_completion_returns_current_fresh_result() {
        let (a_tx, a_rx) = tokio::sync::oneshot::channel();
        let (b_tx, b_rx) = tokio::sync::oneshot::channel();
        let a_probe = Arc::new(ScriptedProbe::new(vec![a_rx]));
        let b_probe = Arc::new(ScriptedProbe::new(vec![b_rx]));
        let cache = Arc::new(ToolHealthCache::new());
        cache.register_probe("tool", a_probe.clone());
        let a_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        a_probe.wait_for_calls(1).await;
        cache.register_probe("tool", b_probe.clone());
        let b_task = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.refresh("tool").await })
        };
        b_probe.wait_for_calls(1).await;

        b_tx.send(unhealthy("b")).unwrap();
        assert_eq!(b_task.await.unwrap(), unhealthy("b"));
        a_tx.send(unhealthy("a")).unwrap();
        assert_eq!(a_task.await.unwrap(), unhealthy("b"));
        assert_eq!(cache.snapshot().reason("tool").unwrap().short_label(), "b");
    }
}
