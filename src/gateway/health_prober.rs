//! Background health prober for circuit-open providers.
//!
//! ## Why this exists
//!
//! The circuit breaker recovers one of two ways: a real request probes the
//! provider once the cooldown elapses (`Open → HalfOpen`), or the operator
//! clicks "Test connection" (`providers.test`, which resets the breaker on a
//! green probe). A provider that sees little traffic can therefore sit
//! circuit-open for the whole cooldown — up to ten minutes after a
//! *permanent* trip — although the endpoint came back long ago. Reference
//! routers run a background prober for exactly this reason (LiteLLM's
//! `background_health_checks`, Bifrost's health-check loop); this is Aleph's.
//!
//! ## Deliberate constraints
//!
//! * **Off by default.** A probe is a real request against a real endpoint —
//!   it costs money and rate budget. The operator opts in with
//!   `[route] health_probe_interval_secs`; `0`/absent means the loop idles.
//!   The knob is read from the live [`RouteHandle`] on every tick, so a
//!   `route_config.update` hot-tunes it with no restart.
//! * **Only circuit-open providers are probed.** A healthy provider is never
//!   sent a probe request — the loop spends nothing while everything works.
//! * **A green probe resets the breaker, nothing else.** Same judgement as
//!   `providers.test` ([`FailoverHealth::reset`]): the ping proves the endpoint
//!   answers; it says nothing about a rate-limit cooldown, so
//!   [`ProviderCooldown`](crate::providers::failover::ProviderCooldown) windows
//!   are deliberately left alone.
//!
//! Lifecycle mirrors [`codex_token_refresher`](super::codex_token_refresher):
//! one process-local `tokio::spawn` loop (the daemon is an OS-level
//! singleton), no shutdown signal — the task lives as long as the process.
//!
//! [`RouteHandle`]: crate::providers::route_handle::RouteHandle

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::config::{Config, ProviderConfig};
use crate::gateway::security::SharedTokenManager;
use crate::providers::failover::FailoverHealth;
use crate::providers::probe::probe_provider_bounded;
use crate::sync_primitives::Arc;

/// Poll cadence while the prober is disabled (`interval == 0`). Cheap and
/// probe-free: the loop must still notice a later `route_config.update`
/// switching the prober on without a restart.
const DISABLED_POLL: Duration = Duration::from_secs(30);

/// The configured probe interval, read fresh from the live route handle.
/// `0` (or no handle wired — tests, pre-boot) means disabled.
fn probe_interval_secs() -> u64 {
    crate::providers::route_handle::try_global_route_handle()
        .map_or(0, |h| h.snapshot().health_probe_interval_secs)
}

/// One probe pass: ping every provider whose circuit is currently **open** and
/// which has a config entry, resetting the breaker for each one that answers.
/// Returns `(provider, probed_ok)` per attempted probe for the caller's logs —
/// and for tests, which drive this directly with a stub `probe`.
///
/// `probe` is injected so the recovery policy is testable without network
/// I/O; production passes [`probe_provider_bounded`].
pub async fn probe_open_circuits_once<F, Fut>(
    health: &FailoverHealth,
    configs: &HashMap<String, ProviderConfig>,
    probe: F,
) -> Vec<(String, bool)>
where
    F: Fn(&str, ProviderConfig) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let open: Vec<String> = health
        .snapshot()
        .await
        .into_iter()
        .filter(|h| h.circuit == "open")
        .map(|h| h.provider)
        .collect();
    let mut outcomes = Vec::new();
    for name in open {
        // An open provider with no config entry (disabled/removed since the
        // trip) is left alone — probing an unconfigured endpoint helps nobody.
        let Some(pc) = configs.get(&name) else {
            debug!(provider = %name, "health probe: open provider not in config; skipping");
            continue;
        };
        // rust-doctor-disable-next-line excessive-clone
        if probe(&name, pc.clone()).await {
            // Same judgement as `providers.test`: the probe proves the endpoint
            // answers, so the breaker resets — and ONLY the breaker (rate-limit
            // pacing windows are untouched; the probe consumed no rate budget).
            health.reset(&name).await;
            outcomes.push((name, true));
        } else {
            outcomes.push((name, false));
        }
    }
    outcomes
}

/// Spawn the background probe loop. Ticks at the configured interval (read
/// fresh every iteration so the knob hot-tunes), idling probe-free while
/// disabled. Spawned once at boot; the loop self-gates, so there is nothing
/// to start or stop later.
pub fn spawn_background(config: Arc<RwLock<Config>>, vault: Arc<SharedTokenManager>) {
    tokio::spawn(async move {
        loop {
            let secs = probe_interval_secs();
            if secs == 0 {
                tokio::time::sleep(DISABLED_POLL).await;
                continue;
            }
            tokio::time::sleep(Duration::from_secs(secs)).await;

            // The shared breaker map the chains mutate — same reachability
            // `providers.test` uses. Absent in tests / before boot wiring.
            let Some(obs) = crate::providers::route_observe::global_route_observability() else {
                continue;
            };
            let open: Vec<String> = obs
                .health
                .snapshot()
                .await
                .into_iter()
                .filter(|h| h.circuit == "open")
                .map(|h| h.provider)
                .collect();
            if open.is_empty() {
                continue; // everything healthy — spend nothing.
            }

            // Snapshot configs + resolve vault keys under the read lock, then
            // release it before any network I/O (same discipline as
            // `providers.healthcheck`).
            let configs: HashMap<String, ProviderConfig> = {
                let cfg = config.read().await;
                open.iter()
                    .filter_map(|name| {
                        cfg.providers.get(name).map(|pc| {
                            let mut runtime = pc.clone();
                            if runtime.api_key.as_ref().is_none_or(|k| k.is_empty()) {
                                runtime.api_key = crate::gateway::handlers::resolve_vault_secret(
                                    &crate::providers::probe::provider_vault_key(name),
                                    &vault,
                                );
                            }
                            // rust-doctor-disable-next-line excessive-clone
                            (name.clone(), runtime)
                        })
                    })
                    .collect()
            };

            let outcomes = probe_open_circuits_once(&obs.health, &configs, |name, pc| {
                let name = name.to_string();
                async move { probe_provider_bounded(&name, pc).await.success }
            })
            .await;
            for (name, ok) in outcomes {
                if ok {
                    info!(provider = %name, "health probe: endpoint answers; circuit reset");
                } else {
                    warn!(provider = %name, "health probe: endpoint still down; circuit stays open");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::Mutex;

    #[tokio::test]
    async fn a_green_probe_resets_only_the_open_provider() {
        let health = FailoverHealth::default();
        health.open_for_test("dead").await;
        let configs = HashMap::from([("dead".to_string(), ProviderConfig::test_config("m"))]);
        let probed = Arc::new(Mutex::new(Vec::new()));
        let probed2 = probed.clone();

        let outcomes = probe_open_circuits_once(&health, &configs, move |name, _| {
            let probed = probed2.clone();
            let name = name.to_string();
            async move {
                // rust-doctor-disable-next-line unwrap-in-production
                probed.lock().unwrap().push(name);
                true
            }
        })
        .await;

        assert_eq!(outcomes, vec![("dead".to_string(), true)]);
        assert!(
            health.snapshot().await.is_empty(),
            "a green probe must reset the breaker"
        );
        // A provider that never tripped is never probed.
        // rust-doctor-disable-next-line unwrap-in-production
        assert_eq!(probed.lock().unwrap().as_slice(), &["dead".to_string()]);
    }

    #[tokio::test]
    async fn a_failed_probe_leaves_the_circuit_open() {
        let health = FailoverHealth::default();
        health.open_for_test("dead").await;
        let configs = HashMap::from([("dead".to_string(), ProviderConfig::test_config("m"))]);

        let outcomes = probe_open_circuits_once(&health, &configs, |_, _| async { false }).await;

        assert_eq!(outcomes, vec![("dead".to_string(), false)]);
        let snap = health.snapshot().await;
        assert_eq!(snap.len(), 1, "a failed probe must not reset the breaker");
        assert_eq!(snap[0].circuit, "open");
    }

    #[tokio::test]
    async fn an_open_provider_without_a_config_entry_is_skipped() {
        // Disabled or removed since the trip: no probe, no reset, no panic.
        let health = FailoverHealth::default();
        health.open_for_test("ghost").await;

        let outcomes = probe_open_circuits_once(&health, &HashMap::new(), |_, _| async {
            panic!("an unconfigured provider must never be probed")
        })
        .await;

        assert!(outcomes.is_empty());
        assert_eq!(health.snapshot().await.len(), 1, "breaker untouched");
    }
}
