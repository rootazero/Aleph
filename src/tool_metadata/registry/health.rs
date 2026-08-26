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
use crate::sync_primitives::{AtomicU64, Ordering};
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

/// In-process TTL cache of probe results. Reads are lock-free via
/// [`ArcSwap`]. Refreshes use [`OnceCell`] single-flight to coalesce
/// concurrent triggers for the same tool.
pub struct ToolHealthCache {
    entries: ArcSwap<HashMap<String, CachedProbe>>,
    probes: ArcSwap<HashMap<String, Arc<dyn ToolHealthProbe>>>,
    /// Single-flight slots keyed by probe name. The map is pruned
    /// explicitly: `invalidate_all` clears it (so a registration-epoch
    /// reset cannot leak stale slots into the next epoch) and
    /// `refresh` removes its own slot on the success path. The Drop
    /// guard inside `refresh` covers cancellation and panic paths so a
    /// dropped future cannot leak the slot for the life of the cache.
    inflight: dashmap::DashMap<String, Arc<OnceCell<ProbeResult>>>,
    generation: AtomicU64,
}

impl ToolHealthCache {
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: ArcSwap::from_pointee(HashMap::new()),
            probes: ArcSwap::from_pointee(HashMap::new()),
            inflight: dashmap::DashMap::new(),
            generation: AtomicU64::new(0),
        }
    }

    /// Register a probe under `name`. Replaces any prior probe for the
    /// same name. Bumps generation so downstream caches (e.g.
    /// `ToolEmitCache`) invalidate on the next read.
    pub fn register_probe(&self, name: impl Into<String>, probe: Arc<dyn ToolHealthProbe>) {
        let name = name.into();
        self.probes.rcu(|probes| {
            let mut next = (**probes).clone();
            next.insert(name.clone(), probe.clone());
            Arc::new(next)
        });
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Remove a probe by name. Bumps generation. Returns `true` if a
    /// probe was actually removed. Also clears any `inflight` slot
    /// for the same name so a re-registration does not inherit a
    /// cancelled or panicked probe future.
    pub fn unregister_probe(&self, name: &str) -> bool {
        let mut removed = false;
        self.probes.rcu(|probes| {
            let mut next = (**probes).clone();
            removed = next.remove(name).is_some();
            Arc::new(next)
        });
        if removed {
            self.inflight.remove(name);
            self.generation.fetch_add(1, Ordering::Release);
        }
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
    /// Does NOT clear registered probes. Also clears the single-flight
    /// `inflight` map so a probe left in flight by a cancelled
    /// `refresh` future cannot survive into the next epoch and block
    /// new triggers for the same name.
    pub fn invalidate_all(&self) {
        self.entries.store(Arc::new(HashMap::new()));
        self.inflight.clear();
        self.generation.fetch_add(1, Ordering::Release);
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

    /// Run the probe with timeout + single-flight, store the result,
    /// bump generation. Returns the freshly-stored result. Does nothing
    /// (returns `Healthy`) if `name` has no registered probe.
    pub async fn refresh(&self, name: &str) -> ProbeResult {
        let probe = match self.probes.load().get(name).cloned() {
            Some(p) => p,
            None => return ProbeResult::Healthy,
        };
        let cell = self
            .inflight
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();
        // RAII guard: if the future is dropped at any await point below
        // (client disconnect, server shutdown, cancellation), the slot
        // is freed. Without this guard, a single cancellation leaks a
        // DashMap entry for the lifetime of the cache, and a probe that
        // panics inside the OnceCell blocks the same name forever (the
        // cell stays empty but is never cleared, so every subsequent
        // refresh re-enters the same panicking initializer).
        struct InflightGuard<'a> {
            cache: &'a ToolHealthCache,
            name: &'a str,
            armed: bool,
        }
        impl<'a> Drop for InflightGuard<'a> {
            fn drop(&mut self) {
                if self.armed {
                    self.cache.inflight.remove(self.name);
                }
            }
        }
        let mut guard = InflightGuard {
            cache: self,
            name,
            armed: true,
        };
        let result = cell
            .get_or_init(|| async {
                let bounded = timeout(PROBE_DEADLINE, probe.probe()).await;
                bounded.unwrap_or(ProbeResult::Unhealthy {
                    reason: HealthReason::DependencyDown(Cow::Borrowed("probe timeout")),
                    retry_after: None,
                })
            })
            .await
            .clone();

        let cached = CachedProbe {
            result: result.clone(),
            cached_at: Instant::now(),
            ttl: probe.ttl(),
        };
        self.entries.rcu(|entries| {
            let mut next = (**entries).clone();
            next.insert(name.to_string(), cached.clone());
            Arc::new(next)
        });
        self.generation.fetch_add(1, Ordering::Release);

        // Disarm the guard before explicit removal so we don't double-free
        // the DashMap entry. The explicit removal stays so concurrent
        // callers that missed the OnceCell still see a fresh entry
        // instead of spawning a redundant probe.
        guard.armed = false;
        self.inflight.remove(name);

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
}
