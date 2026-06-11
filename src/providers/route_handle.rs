//! Live, hot-swappable local/cloud route state.
//!
//! The `[route]` mode ([`crate::config::types::RouteMode`]) is read once at boot
//! into the [`FailoverProvider`](crate::providers::failover::FailoverProvider).
//! A config edit alone never reaches the already-built chain — nothing rebuilds
//! it on `config.reload`. This handle closes that gap: a single process-global,
//! lock-free cell the failover walk reads on *every* request and the
//! `route_config` RPC / `self_config` tool writes on every mode switch. The next
//! prompt sees the new route with **no daemon restart** (the task's hot-state
//! requirement).
//!
//! Lock-free by construction: two atomics, `Relaxed` ordering. Route selection
//! is an advisory infrastructure preference, not a synchronization point — a
//! reader racing a writer simply gets the old-or-new mode for one request, which
//! is exactly the "next prompt picks it up" contract. No mutex on the hot path
//! (the Rust edge over the reference routers' lock-guarded Python state).
//!
//! R7/R10 stance unchanged: this carries the two HARD signals (operator mode +
//! escalation flag) only. It never sees the prompt and never infers intent — it
//! is the same data [`route_policy`](crate::providers::route_policy) already
//! consumed, merely made live instead of boot-frozen.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::OnceLock;

use arc_swap::ArcSwap;

use crate::config::types::{LoadBalanceStrategy, ModelRouteConfig, RouteMode};
use crate::providers::route_policy::{RateLimits, RouteTargets};
use crate::sync_primitives::Arc;

const MODE_AUTO: u8 = 0;
const MODE_ALWAYS_LOCAL: u8 = 1;
const MODE_ALWAYS_CLOUD: u8 = 2;

const LB_ORDERED: u8 = 0;
const LB_ROUND_ROBIN: u8 = 1;
const LB_LEAST_BUSY: u8 = 2;
const LB_LATENCY_AWARE: u8 = 3;
const LB_USAGE_BASED: u8 = 4;

const fn mode_to_u8(mode: RouteMode) -> u8 {
    match mode {
        RouteMode::Auto => MODE_AUTO,
        RouteMode::AlwaysLocal => MODE_ALWAYS_LOCAL,
        RouteMode::AlwaysCloud => MODE_ALWAYS_CLOUD,
    }
}

const fn u8_to_mode(raw: u8) -> RouteMode {
    match raw {
        MODE_ALWAYS_LOCAL => RouteMode::AlwaysLocal,
        MODE_ALWAYS_CLOUD => RouteMode::AlwaysCloud,
        // MODE_AUTO and any out-of-range value fall back to the safe no-op.
        _ => RouteMode::Auto,
    }
}

const fn lb_to_u8(s: LoadBalanceStrategy) -> u8 {
    match s {
        LoadBalanceStrategy::Ordered => LB_ORDERED,
        LoadBalanceStrategy::RoundRobin => LB_ROUND_ROBIN,
        LoadBalanceStrategy::LeastBusy => LB_LEAST_BUSY,
        LoadBalanceStrategy::LatencyAware => LB_LATENCY_AWARE,
        LoadBalanceStrategy::UsageBased => LB_USAGE_BASED,
    }
}

const fn u8_to_lb(raw: u8) -> LoadBalanceStrategy {
    match raw {
        LB_ROUND_ROBIN => LoadBalanceStrategy::RoundRobin,
        LB_LEAST_BUSY => LoadBalanceStrategy::LeastBusy,
        LB_LATENCY_AWARE => LoadBalanceStrategy::LatencyAware,
        LB_USAGE_BASED => LoadBalanceStrategy::UsageBased,
        // LB_ORDERED and any out-of-range value fall back to the safe no-op.
        _ => LoadBalanceStrategy::Ordered,
    }
}

/// Live local/cloud route preference shared between the failover walk (reader)
/// and the config-write path (writer).
///
/// Cheap to clone (it is always behind an [`Arc`]). The two hard scalar signals
/// (mode + escalation) are relaxed atomics; the optional provider pins live in
/// an [`ArcSwap`] so the hot path reads them lock-free too (RCU load, no mutex —
/// the Rust edge over the reference routers' lock-guarded Python state).
#[derive(Debug)]
pub struct RouteHandle {
    mode: AtomicU8,
    allow_escalation: AtomicBool,
    /// Load-balancing strategy for the same-tier fallback pool. A third hard
    /// scalar signal alongside `mode` — same relaxed-atomic, hot-swap contract.
    load_balance: AtomicU8,
    targets: ArcSwap<RouteTargets>,
    /// Per-provider rate ceilings. Same lock-free RCU contract as `targets`: the
    /// hot path reads a stable snapshot, the config-write path swaps a new one.
    limits: ArcSwap<RateLimits>,
}

impl RouteHandle {
    /// Seed a handle from a config snapshot.
    #[must_use]
    pub fn from_config(cfg: &ModelRouteConfig) -> Self {
        Self {
            mode: AtomicU8::new(mode_to_u8(cfg.mode)),
            allow_escalation: AtomicBool::new(cfg.allow_cloud_escalation),
            load_balance: AtomicU8::new(lb_to_u8(cfg.load_balance)),
            targets: ArcSwap::from_pointee(RouteTargets::from_config(cfg)),
            limits: ArcSwap::from_pointee(RateLimits::from_config(cfg)),
        }
    }

    /// Hot-apply a new route preference. Visible to the very next
    /// [`load`](Self::load) / [`targets`](Self::targets) — i.e. the next
    /// request's candidate ordering.
    pub fn store(&self, cfg: &ModelRouteConfig) {
        self.mode.store(mode_to_u8(cfg.mode), Ordering::Relaxed);
        self.allow_escalation
            .store(cfg.allow_cloud_escalation, Ordering::Relaxed);
        self.load_balance
            .store(lb_to_u8(cfg.load_balance), Ordering::Relaxed);
        self.targets.store(Arc::new(RouteTargets::from_config(cfg)));
        self.limits.store(Arc::new(RateLimits::from_config(cfg)));
    }

    /// Read the current `(mode, allow_cloud_escalation)` — the two hard signals
    /// the route policy needs. One relaxed load each; no lock, no await.
    pub fn load(&self) -> (RouteMode, bool) {
        (
            u8_to_mode(self.mode.load(Ordering::Relaxed)),
            self.allow_escalation.load(Ordering::Relaxed),
        )
    }

    /// Read the current load-balancing strategy. One relaxed load; no lock.
    pub fn load_balance(&self) -> LoadBalanceStrategy {
        u8_to_lb(self.load_balance.load(Ordering::Relaxed))
    }

    /// Read the current provider pins. Lock-free RCU load; the returned `Arc` is
    /// a stable snapshot for the duration of one candidate ordering pass.
    pub fn targets(&self) -> Arc<RouteTargets> {
        self.targets.load_full()
    }

    /// Read the current per-provider rate ceilings. Lock-free RCU load; the
    /// returned `Arc` is a stable snapshot for one candidate ordering pass.
    pub fn limits(&self) -> Arc<RateLimits> {
        self.limits.load_full()
    }
}

/// Process-global live handle. The daemon is an OS-level singleton (flock), so
/// one cell per process is the whole world. Set once at boot from the loaded
/// config; mutated in place thereafter via its interior atomics.
static GLOBAL: OnceLock<Arc<RouteHandle>> = OnceLock::new();

/// Get-or-initialise the global handle from `cfg`. The first caller (boot,
/// before the gateway serves) wins the initialisation; later callers get the
/// same `Arc` and ignore their `cfg` argument (the live atomics are the truth).
pub fn global_route_handle(cfg: &ModelRouteConfig) -> Arc<RouteHandle> {
    GLOBAL
        .get_or_init(|| Arc::new(RouteHandle::from_config(cfg)))
        .clone()
}

/// Fetch the global handle if it has already been initialised. The config-write
/// path uses this to hot-apply: present in a running daemon (boot set it),
/// `None` only before boot wiring (then the on-disk write still takes effect at
/// the next start).
pub fn try_global_route_handle() -> Option<Arc<RouteHandle>> {
    GLOBAL.get().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_config_round_trips_each_mode() {
        for (mode, esc) in [
            (RouteMode::Auto, false),
            (RouteMode::AlwaysLocal, true),
            (RouteMode::AlwaysCloud, false),
        ] {
            let cfg = ModelRouteConfig {
                mode,
                allow_cloud_escalation: esc,
                ..Default::default()
            };
            let h = RouteHandle::from_config(&cfg);
            assert_eq!(h.load(), (mode, esc));
        }
    }

    #[test]
    fn store_hot_applies() {
        let h = RouteHandle::from_config(&ModelRouteConfig::default());
        assert_eq!(h.load(), (RouteMode::Auto, false));

        h.store(&ModelRouteConfig {
            mode: RouteMode::AlwaysLocal,
            allow_cloud_escalation: true,
            ..Default::default()
        });
        assert_eq!(h.load(), (RouteMode::AlwaysLocal, true));

        h.store(&ModelRouteConfig {
            mode: RouteMode::AlwaysCloud,
            allow_cloud_escalation: false,
            ..Default::default()
        });
        assert_eq!(h.load(), (RouteMode::AlwaysCloud, false));
    }

    #[test]
    fn targets_default_empty_and_hot_apply() {
        let h = RouteHandle::from_config(&ModelRouteConfig::default());
        assert_eq!(*h.targets(), RouteTargets::default());

        h.store(&ModelRouteConfig {
            mode: RouteMode::AlwaysLocal,
            local_provider: Some("ollama".to_string()),
            cloud_provider: Some("anthropic".to_string()),
            ..Default::default()
        });
        let t = h.targets();
        assert_eq!(t.local_provider.as_deref(), Some("ollama"));
        assert_eq!(t.cloud_provider.as_deref(), Some("anthropic"));
        assert!(t.is_pinned("ollama"));
        assert!(t.is_pinned("anthropic"));
        assert!(!t.is_pinned("openai"));

        // Clearing pins hot-applies back to empty.
        h.store(&ModelRouteConfig::default());
        assert_eq!(*h.targets(), RouteTargets::default());
    }

    #[test]
    fn unknown_raw_mode_decodes_to_auto() {
        assert_eq!(u8_to_mode(99), RouteMode::Auto);
    }

    #[test]
    fn unknown_raw_lb_decodes_to_ordered() {
        assert_eq!(u8_to_lb(99), LoadBalanceStrategy::Ordered);
    }

    #[test]
    fn usage_based_lb_round_trips() {
        assert_eq!(lb_to_u8(LoadBalanceStrategy::UsageBased), LB_USAGE_BASED);
        assert_eq!(u8_to_lb(LB_USAGE_BASED), LoadBalanceStrategy::UsageBased);
    }

    #[test]
    fn limits_default_empty_and_hot_apply() {
        use crate::config::types::ProviderRateLimit;

        let h = RouteHandle::from_config(&ModelRouteConfig::default());
        assert!(h.limits().is_empty());

        h.store(&ModelRouteConfig {
            rate_limits: [(
                "anthropic".to_string(),
                ProviderRateLimit {
                    rpm: Some(60),
                    tpm: Some(90_000),
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        });
        let lim = h.limits();
        assert!(!lim.is_empty());
        // 30/60 rpm = 500‰, 0 tpm → binding = 500‰, not over.
        assert_eq!(lim.assess("anthropic", 30, 0), (500, false));

        // Clearing hot-applies back to empty.
        h.store(&ModelRouteConfig::default());
        assert!(h.limits().is_empty());
    }

    #[test]
    fn load_balance_defaults_and_hot_applies() {
        let h = RouteHandle::from_config(&ModelRouteConfig::default());
        assert_eq!(h.load_balance(), LoadBalanceStrategy::Ordered);

        for s in [
            LoadBalanceStrategy::RoundRobin,
            LoadBalanceStrategy::LeastBusy,
            LoadBalanceStrategy::LatencyAware,
            LoadBalanceStrategy::Ordered,
        ] {
            h.store(&ModelRouteConfig {
                load_balance: s,
                ..Default::default()
            });
            assert_eq!(h.load_balance(), s);
        }
    }
}
