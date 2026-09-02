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
//!   The knob is re-read from the live [`RouteHandle`] at every slice of the
//!   wait (see `wait_for_next_pass`), so a `route_config.update` hot-tunes it
//!   with no restart — including switching it off before the pass it was
//!   already waiting on.
//! * **Only circuit-open providers the operator left enabled are probed.** A
//!   healthy provider is never sent a probe request — the loop spends nothing
//!   while everything works — and a provider the operator switched off is not
//!   dialled at all (its breaker stays open until they switch it back on).
//!   `supports_health_check = false` presets are still probed: that flag says
//!   `GET /models` 404s, and this loop dials a `ping` completion, so honouring
//!   it here would strand those breakers open with no operator exit.
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
use crate::providers::probe::{probe_disposition, probe_provider_bounded, ProbeDisposition};
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

/// One probe pass: ping every provider whose circuit is currently **open**,
/// which has a config entry, and which the operator has left enabled —
/// resetting the breaker for each one that answers. Returns
/// `(provider, probed_ok)` per attempted probe for the caller's logs — and for
/// tests, which drive this directly with a stub `probe`.
///
/// The enabled check goes through [`probe_disposition`] — the one derivation
/// of "may I dial this provider", shared with every other face that asks it,
/// so the answer cannot differ by surface (the census lives in
/// `providers::probe`, not in a list here). Only its
/// [`ProbeDisposition::Disabled`] arm is honoured here:
/// [`ProbeDisposition::Unsupported`] means the preset's `GET /models` 404s,
/// and this loop dials a `ping` completion instead — suppressing those would
/// be a gate with no way for the operator to open it.
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
        // An open provider with no config entry (removed since the trip) is
        // left alone — probing an unconfigured endpoint helps nobody. A
        // *disabled* provider does still have a config entry, which is why the
        // operator switch is a second, explicit question below.
        let Some(pc) = configs.get(&name) else {
            debug!(provider = %name, "health probe: open provider not in config; skipping");
            continue;
        };
        if probe_disposition(&name, pc.enabled) == ProbeDisposition::Disabled {
            debug!(provider = %name, "health probe: provider disabled by operator; skipping");
            continue;
        }
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

/// Sleep until the next probe pass is due, re-reading `interval` after every
/// [`DISABLED_POLL`] slice. Sleeping the whole interval in one call would read
/// the knob only *before* the wait, so a `route_config.update` switching the
/// prober off mid-interval still bought one more full paid pass; `0` at any
/// slice boundary discards the wait accumulated so far.
async fn wait_for_next_pass<F: Fn() -> u64>(interval: F) {
    let mut waited = Duration::ZERO;
    loop {
        let secs = interval();
        if secs == 0 {
            waited = Duration::ZERO;
            tokio::time::sleep(DISABLED_POLL).await;
            continue;
        }
        let Some(remaining) = Duration::from_secs(secs).checked_sub(waited) else {
            return; // the interval was tuned down below what we already waited
        };
        if remaining.is_zero() {
            return;
        }
        let slice = remaining.min(DISABLED_POLL);
        tokio::time::sleep(slice).await;
        waited += slice;
    }
}

/// Spawn the background probe loop. Ticks at the configured interval (read
/// fresh every slice of the wait, so the knob hot-tunes in both directions),
/// idling probe-free while disabled. Spawned once at boot; the loop self-gates,
/// so there is nothing to start or stop later.
pub fn spawn_background(config: Arc<RwLock<Config>>, vault: Arc<SharedTokenManager>) {
    tokio::spawn(async move {
        loop {
            wait_for_next_pass(probe_interval_secs).await;

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

    /// The operator's switch is the one that suppresses a probe. A disabled
    /// provider keeps its config entry, so `configs.get` waves it through; the
    /// loop would otherwise send a real, paid `ping` completion to an endpoint
    /// the operator turned off, every tick, forever.
    #[tokio::test]
    async fn a_disabled_provider_with_an_open_circuit_is_never_probed() {
        let health = FailoverHealth::default();
        health.open_for_test("off").await;
        let mut pc = ProviderConfig::test_config("m");
        pc.enabled = false;
        let configs = HashMap::from([("off".to_string(), pc)]);

        let outcomes = probe_open_circuits_once(&health, &configs, |_, _| async {
            panic!("a provider the operator disabled must never be dialled")
        })
        .await;

        assert!(outcomes.is_empty());
        assert_eq!(health.snapshot().await.len(), 1, "breaker untouched");
    }

    /// ...and only that arm. `chatgpt` is one of the six presets that declare
    /// `supports_health_check = false` (OAuth-only / per-deployment hosts whose
    /// `GET /models` 404s), but this loop dials a `ping` completion, so the
    /// opt-out says nothing about it. Filtering on `Unsupported` would leave
    /// those providers' breakers open until the operator hand-clicked "Test
    /// connection" — a gate with no exit.
    #[tokio::test]
    async fn an_unsupported_preset_is_still_probed() {
        assert_eq!(
            probe_disposition("chatgpt", true),
            ProbeDisposition::Unsupported,
            "this test is only meaningful while chatgpt opts out of /models probing"
        );
        let health = FailoverHealth::default();
        health.open_for_test("chatgpt").await;
        let configs = HashMap::from([("chatgpt".to_string(), ProviderConfig::test_config("m"))]);

        let outcomes = probe_open_circuits_once(&health, &configs, |_, _| async { true }).await;

        assert_eq!(outcomes, vec![("chatgpt".to_string(), true)]);
        assert!(health.snapshot().await.is_empty(), "breaker reset");
    }

    /// Turning the prober off mid-interval must suppress the pass it was
    /// already waiting on, not the one after it: the old loop read the knob
    /// once and then slept the whole interval, so a disable still bought one
    /// more full paid pass (up to the configured interval late).
    #[tokio::test(start_paused = true)]
    async fn a_disable_landing_mid_interval_suppresses_the_next_pass() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let knob = Arc::new(AtomicU64::new(600));
        let reader = knob.clone();
        let waiter = tokio::spawn(async move {
            wait_for_next_pass(move || reader.load(Ordering::Relaxed)).await;
        });

        tokio::time::sleep(Duration::from_secs(90)).await;
        assert!(!waiter.is_finished(), "600s has not elapsed yet");
        knob.store(0, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_secs(900)).await;
        assert!(
            !waiter.is_finished(),
            "a disable must cancel the pending pass, not just the following one"
        );

        knob.store(1, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_secs(120)).await;
        assert!(
            waiter.is_finished(),
            "re-enabling must resume without a restart"
        );
    }

    #[tokio::test]
    async fn an_open_provider_without_a_config_entry_is_skipped() {
        // Removed from the config since the trip: no probe, no reset, no panic.
        // (A *disabled* provider still has an entry — see the test above.)
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
