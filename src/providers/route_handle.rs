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
//! Lock-free by construction: the whole route state lives behind one
//! [`ArcSwap`] cell. A hot-apply publishes *every* field atomically (a single
//! RCU swap) and the failover walk reads one coherent [`snapshot`](RouteHandle::snapshot)
//! per candidate-ordering pass — so a reader racing a writer gets the whole old
//! config or the whole new one for one request, never a torn mix (new mode with
//! stale targets). "Next prompt picks it up" without a mutex on the hot path
//! (the Rust edge over the reference routers' lock-guarded Python state).
//!
//! R7/R10 stance unchanged: this carries the two HARD signals (operator mode +
//! escalation flag) only. It never sees the prompt and never infers intent — it
//! is the same data [`route_policy`](crate::providers::route_policy) already
//! consumed, merely made live instead of boot-frozen.

use std::sync::OnceLock;

use arc_swap::ArcSwap;

use crate::config::types::{LoadBalanceStrategy, ModelRouteConfig, RouteMode};
use crate::providers::route_policy::{RateLimits, RouteTargets};
use crate::sync_primitives::Arc;

/// One coherent generation of the live route state.
///
/// Published as a whole via [`RouteHandle::store`] and read as a whole via
/// [`RouteHandle::snapshot`], so the failover walk never observes a torn mix of
/// fields from two different config generations.
#[derive(Debug, Clone)]
pub struct RouteState {
    /// Operator local/cloud preference.
    pub mode: RouteMode,
    /// Whether an `AlwaysLocal` route may still escalate to cloud.
    pub allow_escalation: bool,
    /// Load-balancing strategy for the same-tier fallback pool.
    pub load_balance: LoadBalanceStrategy,
    /// Operator provider pins.
    pub targets: Arc<RouteTargets>,
    /// Per-provider rate ceilings.
    pub limits: Arc<RateLimits>,
    /// Background health-probe interval for circuit-open providers, in
    /// seconds. `0` = disabled (the default — probing spends real requests).
    /// Read by the gateway health prober on every tick, so a
    /// `route_config.update` hot-tunes it without a restart.
    pub health_probe_interval_secs: u64,
}

impl RouteState {
    fn from_config(cfg: &ModelRouteConfig) -> Self {
        Self {
            mode: cfg.mode,
            allow_escalation: cfg.allow_cloud_escalation,
            load_balance: cfg.load_balance,
            targets: Arc::new(RouteTargets::from_config(cfg)),
            limits: Arc::new(RateLimits::from_config(cfg)),
            health_probe_interval_secs: cfg.health_probe_interval_secs.unwrap_or(0),
        }
    }
}

/// Live local/cloud route preference shared between the failover walk (reader)
/// and the config-write path (writer).
///
/// Cheap to clone (it is always behind an [`Arc`]). The whole [`RouteState`]
/// lives in one [`ArcSwap`] so the hot path reads a coherent snapshot lock-free
/// (RCU load, no mutex — the Rust edge over the reference routers' lock-guarded
/// Python state) and the config-write path swaps a new generation atomically.
#[derive(Debug)]
pub struct RouteHandle {
    state: ArcSwap<RouteState>,
}

impl RouteHandle {
    /// Seed a handle from a config snapshot.
    #[must_use]
    pub fn from_config(cfg: &ModelRouteConfig) -> Self {
        Self {
            state: ArcSwap::from_pointee(RouteState::from_config(cfg)),
        }
    }

    /// Hot-apply a new route preference as a single atomic publish. Visible to
    /// the very next [`snapshot`](Self::snapshot) — i.e. the next request's
    /// candidate ordering — as one coherent generation: a concurrent reader gets
    /// the whole old config or the whole new one, never a torn mix.
    pub fn store(&self, cfg: &ModelRouteConfig) {
        self.state.store(Arc::new(RouteState::from_config(cfg)));
    }

    /// One coherent snapshot of the live route state (single lock-free RCU load).
    /// The failover walk reads every field it needs from this so one
    /// candidate-ordering pass sees a single config generation.
    pub fn snapshot(&self) -> Arc<RouteState> {
        self.state.load_full()
    }
}

/// Process-global live handle. The daemon is an OS-level singleton (flock), so
/// one cell per process is the whole world. Set once at boot from the loaded
/// config; swapped in place thereafter via its interior [`ArcSwap`].
///
/// # Deliberately NOT a `CapabilitySlot` — adjudicated 2026-08-25
///
/// The capability-wiring round migrated 45 process-global handles onto
/// [`crate::capability::CapabilitySlot`], so that "never installed" stops being
/// indistinguishable from "installed with this value". This static is the one
/// member that stayed raw, and the reason is in [`global_route_handle`] just
/// below: it is **first-caller-wins over a `cfg` parameter**. `get_or_init`
/// builds the state from whichever caller arrives first, and every later caller
/// hands over a `cfg` that is ignored on purpose — the live state, hot-swapped
/// since boot, is the truth.
///
/// `CapabilitySlot::install` takes the value, not a thunk over the caller's
/// argument. Migrating would therefore mean either changing the slot's
/// `Outcome` shape to carry a builder, or changing this call site so the value
/// is computed before the race is decided — and that second one changes WHICH
/// caller's config wins. That is a behaviour change wearing a refactor's
/// clothes, on the path every request's failover walk reads.
///
/// So: leave it raw, and do not read `1 raw, 45 slots` in the census as
/// unfinished work. The census names this static explicitly rather than
/// swallowing it in a broad predicate — see
/// `capability::census::every_installed_global_is_a_capability_slot` (whose
/// third assertion pins the exemption at exactly one member, by name) and
/// `capability::census::route_handle_global_is_selected_by_the_first_caller_wins_arm_alone`
/// (which proves it is still chosen by that arm and by nothing else). A second
/// raw handle appearing anywhere still fails that guard, as it should.
static GLOBAL: OnceLock<Arc<RouteHandle>> = OnceLock::new();

/// Get-or-initialise the global handle from `cfg`. The first caller (boot,
/// before the gateway serves) wins the initialisation; later callers get the
/// same `Arc` and ignore their `cfg` argument (the live state is the truth).
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
            assert_eq!(
                {
                    let s = h.snapshot();
                    (s.mode, s.allow_escalation)
                },
                (mode, esc)
            );
        }
    }

    #[test]
    fn store_hot_applies() {
        let h = RouteHandle::from_config(&ModelRouteConfig::default());
        assert_eq!(
            {
                let s = h.snapshot();
                (s.mode, s.allow_escalation)
            },
            (RouteMode::Auto, false)
        );

        h.store(&ModelRouteConfig {
            mode: RouteMode::AlwaysLocal,
            allow_cloud_escalation: true,
            ..Default::default()
        });
        assert_eq!(
            {
                let s = h.snapshot();
                (s.mode, s.allow_escalation)
            },
            (RouteMode::AlwaysLocal, true)
        );

        h.store(&ModelRouteConfig {
            mode: RouteMode::AlwaysCloud,
            allow_cloud_escalation: false,
            ..Default::default()
        });
        assert_eq!(
            {
                let s = h.snapshot();
                (s.mode, s.allow_escalation)
            },
            (RouteMode::AlwaysCloud, false)
        );
    }

    #[test]
    fn snapshot_carries_every_field_coherently() {
        let h = RouteHandle::from_config(&ModelRouteConfig::default());
        let s = h.snapshot();
        assert_eq!(s.mode, RouteMode::Auto);
        assert!(!s.allow_escalation);
        assert_eq!(s.load_balance, LoadBalanceStrategy::Ordered);
        assert_eq!(*s.targets, RouteTargets::default());
        assert!(s.limits.is_empty());

        h.store(&ModelRouteConfig {
            mode: RouteMode::AlwaysCloud,
            allow_cloud_escalation: true,
            load_balance: LoadBalanceStrategy::LeastBusy,
            cloud_provider: Some("anthropic".to_string()),
            ..Default::default()
        });
        // A snapshot taken before the store still reads the whole old generation.
        assert_eq!(s.mode, RouteMode::Auto);
        assert_eq!(*s.targets, RouteTargets::default());
        // A fresh snapshot reads the whole new generation.
        let s2 = h.snapshot();
        assert_eq!(s2.mode, RouteMode::AlwaysCloud);
        assert!(s2.allow_escalation);
        assert_eq!(s2.load_balance, LoadBalanceStrategy::LeastBusy);
        assert!(s2.targets.is_pinned("anthropic"));
    }

    #[test]
    fn targets_default_empty_and_hot_apply() {
        let h = RouteHandle::from_config(&ModelRouteConfig::default());
        assert_eq!(*h.snapshot().targets, RouteTargets::default());

        h.store(&ModelRouteConfig {
            mode: RouteMode::AlwaysLocal,
            local_provider: Some("ollama".to_string()),
            cloud_provider: Some("anthropic".to_string()),
            ..Default::default()
        });
        let snap = h.snapshot();
        let t = &snap.targets;
        assert_eq!(t.local_provider.as_deref(), Some("ollama"));
        assert_eq!(t.cloud_provider.as_deref(), Some("anthropic"));
        assert!(t.is_pinned("ollama"));
        assert!(t.is_pinned("anthropic"));
        assert!(!t.is_pinned("openai"));

        // Clearing pins hot-applies back to empty.
        h.store(&ModelRouteConfig::default());
        assert_eq!(*h.snapshot().targets, RouteTargets::default());
    }

    #[test]
    fn limits_default_empty_and_hot_apply() {
        use crate::config::types::ProviderRateLimit;

        let h = RouteHandle::from_config(&ModelRouteConfig::default());
        assert!(h.snapshot().limits.is_empty());

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
        let snap = h.snapshot();
        let lim = &snap.limits;
        assert!(!lim.is_empty());
        // 30/60 rpm = 500‰, 0 tpm → binding = 500‰, not over.
        assert_eq!(lim.assess("anthropic", 30, 0), (500, false));

        // Clearing hot-applies back to empty.
        h.store(&ModelRouteConfig::default());
        assert!(h.snapshot().limits.is_empty());
    }

    #[test]
    fn load_balance_defaults_and_hot_applies() {
        let h = RouteHandle::from_config(&ModelRouteConfig::default());
        assert_eq!(h.snapshot().load_balance, LoadBalanceStrategy::Ordered);

        for s in [
            LoadBalanceStrategy::RoundRobin,
            LoadBalanceStrategy::LeastBusy,
            LoadBalanceStrategy::LatencyAware,
            LoadBalanceStrategy::CostAware,
            LoadBalanceStrategy::Ordered,
        ] {
            h.store(&ModelRouteConfig {
                load_balance: s,
                ..Default::default()
            });
            assert_eq!(h.snapshot().load_balance, s);
        }
    }

    /// A config hot-swap that flips mode *and* pins together must never be
    /// observed half-applied. Two coherent configs are alternated by a writer
    /// while readers assert every observation is fully-A or fully-B — never a
    /// torn mix (new mode + old targets). The single-`ArcSwap` publish plus a
    /// one-shot [`RouteHandle::snapshot`] per read guarantees coherence; the
    /// pre-fix per-field accessors (`load()` then `targets()`) tore here.
    #[test]
    fn concurrent_store_is_never_observed_torn() {
        use std::sync::atomic::{AtomicBool, Ordering as O};
        use std::sync::Arc as StdArc;

        // Config A: always-local, pinned to "ollama", no cloud pin.
        let cfg_a = ModelRouteConfig {
            mode: RouteMode::AlwaysLocal,
            local_provider: Some("ollama".to_string()),
            ..Default::default()
        };
        // Config B: always-cloud, pinned to "anthropic", no local pin.
        let cfg_b = ModelRouteConfig {
            mode: RouteMode::AlwaysCloud,
            cloud_provider: Some("anthropic".to_string()),
            ..Default::default()
        };

        let handle = StdArc::new(RouteHandle::from_config(&cfg_a));
        let stop = StdArc::new(AtomicBool::new(false));

        let writer = {
            let handle = StdArc::clone(&handle);
            std::thread::spawn(move || {
                for i in 0..200_000u32 {
                    handle.store(if i % 2 == 0 { &cfg_b } else { &cfg_a });
                }
            })
        };

        let readers: Vec<_> = (0..3)
            .map(|_| {
                let handle = StdArc::clone(&handle);
                let stop = StdArc::clone(&stop);
                std::thread::spawn(move || {
                    while !stop.load(O::Relaxed) {
                        // One coherent snapshot — all fields share one generation.
                        let snap = handle.snapshot();
                        match snap.mode {
                            RouteMode::AlwaysLocal => {
                                assert!(
                                    snap.targets.is_pinned("ollama")
                                        && !snap.targets.is_pinned("anthropic"),
                                    "torn read: AlwaysLocal mode with non-A targets"
                                );
                            }
                            RouteMode::AlwaysCloud => {
                                assert!(
                                    snap.targets.is_pinned("anthropic")
                                        && !snap.targets.is_pinned("ollama"),
                                    "torn read: AlwaysCloud mode with non-B targets"
                                );
                            }
                            RouteMode::Auto => unreachable!("neither config uses Auto"),
                        }
                    }
                })
            })
            .collect();

        writer.join().unwrap();
        stop.store(true, O::Relaxed);
        for r in readers {
            r.join().unwrap();
        }
    }
}
