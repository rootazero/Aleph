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

use crate::config::types::{ModelRouteConfig, RouteMode};
use crate::sync_primitives::Arc;

const MODE_AUTO: u8 = 0;
const MODE_ALWAYS_LOCAL: u8 = 1;
const MODE_ALWAYS_CLOUD: u8 = 2;

fn mode_to_u8(mode: RouteMode) -> u8 {
    match mode {
        RouteMode::Auto => MODE_AUTO,
        RouteMode::AlwaysLocal => MODE_ALWAYS_LOCAL,
        RouteMode::AlwaysCloud => MODE_ALWAYS_CLOUD,
    }
}

fn u8_to_mode(raw: u8) -> RouteMode {
    match raw {
        MODE_ALWAYS_LOCAL => RouteMode::AlwaysLocal,
        MODE_ALWAYS_CLOUD => RouteMode::AlwaysCloud,
        // MODE_AUTO and any out-of-range value fall back to the safe no-op.
        _ => RouteMode::Auto,
    }
}

/// Live local/cloud route preference shared between the failover walk (reader)
/// and the config-write path (writer).
///
/// Cheap to clone (it is always behind an [`Arc`]); reads and writes are single
/// relaxed atomic ops.
#[derive(Debug)]
pub struct RouteHandle {
    mode: AtomicU8,
    allow_escalation: AtomicBool,
}

impl RouteHandle {
    /// Seed a handle from a config snapshot.
    pub fn from_config(cfg: &ModelRouteConfig) -> Self {
        Self {
            mode: AtomicU8::new(mode_to_u8(cfg.mode)),
            allow_escalation: AtomicBool::new(cfg.allow_cloud_escalation),
        }
    }

    /// Hot-apply a new route preference. Visible to the very next
    /// [`load`](Self::load) — i.e. the next request's candidate ordering.
    pub fn store(&self, cfg: &ModelRouteConfig) {
        self.mode.store(mode_to_u8(cfg.mode), Ordering::Relaxed);
        self.allow_escalation
            .store(cfg.allow_cloud_escalation, Ordering::Relaxed);
    }

    /// Read the current `(mode, allow_cloud_escalation)` — the two hard signals
    /// the route policy needs. One relaxed load each; no lock, no await.
    pub fn load(&self) -> (RouteMode, bool) {
        (
            u8_to_mode(self.mode.load(Ordering::Relaxed)),
            self.allow_escalation.load(Ordering::Relaxed),
        )
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
        });
        assert_eq!(h.load(), (RouteMode::AlwaysLocal, true));

        h.store(&ModelRouteConfig {
            mode: RouteMode::AlwaysCloud,
            allow_cloud_escalation: false,
        });
        assert_eq!(h.load(), (RouteMode::AlwaysCloud, false));
    }

    #[test]
    fn unknown_raw_mode_decodes_to_auto() {
        assert_eq!(u8_to_mode(99), RouteMode::Auto);
    }
}
