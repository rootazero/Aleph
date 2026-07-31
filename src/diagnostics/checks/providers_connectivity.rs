//! `providers/connectivity` — live LLM provider reachability.
//!
//! Runtime sibling of the path-based core checks. The `aleph doctor` CLI
//! consumes these findings through the daemon-side engine over the
//! `diagnostics.run` RPC (it no longer probes `providers.healthcheck`
//! directly), and the LLM-facing `doctor` tool runs the same check — so
//! after the `[f]` AI-repair flow fixes a provider credential (via
//! `vault_store` / `self_config`), the agent can verify the provider is
//! actually reachable again. The check reuses the same bounded probe as the
//! `providers.healthcheck` RPC handler
//! ([`crate::providers::probe::probe_provider_bounded`] — one source of truth)
//! against the live daemon config + vault handles.
//!
//! Read-only and **non-repairable**: an unreachable provider is a credential
//! or network condition; the fix is an LLM/user action (store a fresh key,
//! correct the provider section), never a mechanical doctor repair. Findings
//! carry a `fix_hint` routing to those tools.
//!
//! Registered only via [`DiagnosticEngine::with_runtime_checks`]
//! (`super::super`) — the offline `default_registry()` stays path-only so
//! `aleph-server doctor` keeps working without network access.

use async_trait::async_trait;
use futures::future::join_all;
use tokio::sync::RwLock;

use crate::config::{Config, ProviderConfig};
use crate::diagnostics::check::{HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, Severity};
use crate::diagnostics::redact::redact_secrets;
use crate::gateway::security::SharedTokenManager;
use crate::providers::probe::{probe_provider_bounded, provider_vault_key};
use crate::sync_primitives::Arc;

const ID: &str = "providers/connectivity";

/// Tag on the per-provider success finding — the total-outage gate counts
/// these (not title strings) to decide whether any probed provider answered.
const TAG_REACHABLE: &str = "provider-reachable";
/// Tag on per-provider failure findings (including timeouts, which the
/// bounded probe folds into ordinary failures).
const TAG_UNREACHABLE: &str = "provider-unreachable";

pub struct ProvidersConnectivityCheck {
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
}

impl ProvidersConnectivityCheck {
    pub const fn new(config: Arc<RwLock<Config>>, vault: Arc<SharedTokenManager>) -> Self {
        Self { config, vault }
    }

    /// Resolve a provider's API key from the vault (`ai:<name>`). Mirrors the
    /// gateway handler's resolution so both surfaces probe identical configs.
    fn resolve_key(&self, name: &str) -> Option<String> {
        match self.vault.get_secret(&provider_vault_key(name)) {
            Ok(Some(secret)) => Some(secret.expose().to_string()),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(provider = %name, error = %e, "vault read failed during connectivity probe");
                None
            }
        }
    }
}

#[async_trait]
impl HealthCheck for ProvidersConnectivityCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Provider connectivity"
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        // Snapshot configs + resolve keys under the read lock, then release
        // it before any network I/O (same discipline as `providers.healthcheck`).
        let probes: Vec<(String, bool, ProviderConfig)> = {
            let cfg = self.config.read().await;
            cfg.providers
                .iter()
                .map(|(name, pc)| {
                    let mut runtime = pc.clone();
                    runtime.api_key = self.resolve_key(name);
                    (name.clone(), pc.enabled, runtime)
                })
                .collect()
        };

        if probes.is_empty() {
            return vec![Finding::ok(
                ID,
                "No providers configured",
                "No LLM providers in config.toml; nothing to probe.",
            )];
        }

        // Count providers that will actually be probed (enabled AND whose
        // preset opts in to `/models` health probing) up-front — `probes` is
        // consumed below. The post-join total-outage gate keys off this so a
        // fleet of OAuth-only / placeholder providers that all SKIP probing is
        // never mistaken for a total outage.
        let probed_count = probes
            .iter()
            .filter(|(name, enabled, _)| {
                *enabled
                    && crate::providers::presets::get_preset(name)
                        .is_none_or(|p| p.supports_health_check)
            })
            .count();
        let futures = probes
            .into_iter()
            .map(|(name, enabled, runtime)| async move {
                if !enabled {
                    return Finding::ok(
                        ID,
                        format!("{name}: disabled"),
                        "Provider is disabled; not probed.",
                    );
                }
                // Honor the preset's `supports_health_check` opt-out: OAuth-only
                // or placeholder-URL providers 404 / rate-limit on `/models`, so
                // probing them yields a false `unreachable`. Report `skipped`
                // (Info) instead — and the outage gate ignores them.
                if !crate::providers::presets::get_preset(&name)
                    .is_none_or(|p| p.supports_health_check)
                {
                    return Finding::ok(
                        ID,
                        format!("{name}: probe skipped"),
                        "Preset opts out of /models health probing (OAuth-only or \
                         placeholder endpoint); credentials not verifiable here.",
                    );
                }
                let outcome = probe_provider_bounded(&name, runtime).await;
                if outcome.success {
                    let lat = outcome
                        .latency_ms
                        .map(|m| format!(" ({m}ms)"))
                        .unwrap_or_default();
                    Finding::ok(ID, format!("{name}: reachable"), format!("ping ok{lat}"))
                        .with_tag(TAG_REACHABLE)
                } else {
                    // Provider errors can echo credentials (Authorization
                    // headers, keyed URLs) — redact before the text reaches
                    // findings (CLI, --json, LLM tool output).
                    Finding::problem(
                        ID,
                        Severity::Warning,
                        format!("{name}: unreachable"),
                        redact_secrets(
                            &outcome
                                .error
                                .unwrap_or_else(|| "unknown probe error".to_string()),
                        ),
                    )
                    .with_fix_hint(unreachable_hint(&name))
                    .with_tag(TAG_UNREACHABLE)
                }
            });

        let mut findings = join_all(futures).await;
        findings.sort_by(|a, b| a.title.cmp(&b.title));

        // Total-outage gate: if every PROBED provider failed (no
        // `provider-reachable` finding emitted) the operator has zero working
        // LLM access — a doctor-gating Error, not a pile of per-provider
        // Warnings whose aggregate still exits `ok`. Disabled and
        // probe-skipped providers short-circuit before the probe, so the
        // absence of any reachable tag when at least one provider was
        // actually probed means a complete outage. Counted by TAG, not by
        // title string, so reworded titles can't silently break the gate.
        if total_outage(probed_count, &findings) {
            findings.insert(
                0,
                Finding::problem(
                    ID,
                    Severity::Error,
                    "No providers reachable",
                    "Every enabled LLM provider failed its connectivity probe — \
                     the agent has no working model access.",
                )
                .with_fix_hint(
                    "Restore at least one provider: refresh its key with vault_store \
                     or fix its `providers.*` section via self_config, then re-run doctor.",
                ),
            );
        }
        findings
    }
}

/// Total-outage decision: at least one provider was actually probed and none
/// of them answered (no finding carries the `provider-reachable` tag).
fn total_outage(probed_count: usize, findings: &[Finding]) -> bool {
    probed_count > 0 && !findings.iter().any(|f| f.has_tag(TAG_REACHABLE))
}

/// Route the LLM (or user) to the tools that can actually fix an unreachable
/// provider, and close the loop by re-running doctor afterwards.
fn unreachable_hint(name: &str) -> String {
    format!(
        "Refresh the API key with vault_store (vault key `ai:{name}`) or fix the \
         `providers.{name}` section via self_config, then re-run doctor to verify."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::store::SecurityStore;
    use tempfile::TempDir;

    fn empty_check() -> (ProvidersConnectivityCheck, TempDir) {
        let tmp = TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let mgr = SharedTokenManager::new(store, tmp.path().join("vault"));
        let check = ProvidersConnectivityCheck::new(
            Arc::new(RwLock::new(Config::default())),
            Arc::new(mgr),
        );
        (check, tmp)
    }

    #[tokio::test]
    async fn ok_when_no_providers_configured() {
        let (check, _tmp) = empty_check();
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings.len(), 1);
        assert!(!findings[0].is_problem());
        assert_eq!(findings[0].check_id, ID);
    }

    #[tokio::test]
    async fn never_repairs_in_fix_posture() {
        // Connectivity is never mechanically repairable — even Fix posture
        // must stay read-only.
        let (check, _tmp) = empty_check();
        let findings = check.run(Posture::Fix).await;
        assert!(findings.iter().all(|f| !f.repairable));
        assert!(findings.iter().all(|f| f.repair_outcome.is_none()));
    }

    #[test]
    fn hint_routes_to_vault_store_and_self_config() {
        let hint = unreachable_hint("openai");
        assert!(hint.contains("vault_store"));
        assert!(hint.contains("ai:openai"));
        assert!(hint.contains("self_config"));
        assert!(hint.contains("re-run doctor"));
    }

    #[test]
    fn outage_gate_counts_reachable_tags_not_titles() {
        let reachable =
            vec![Finding::ok(ID, "openai: reachable", "ping ok").with_tag(TAG_REACHABLE)];
        let unreachable =
            vec![
                Finding::problem(ID, Severity::Warning, "openai: unreachable", "boom")
                    .with_tag(TAG_UNREACHABLE),
            ];
        // A reachable provider suppresses the gate, whatever its title says.
        assert!(!total_outage(1, &reachable));
        // All probed providers failed → total outage.
        assert!(total_outage(1, &unreachable));
        assert!(total_outage(3, &unreachable));
        // Nothing was actually probed (all disabled/skipped) → never an outage.
        assert!(!total_outage(0, &unreachable));
        assert!(!total_outage(0, &[]));
        // A finding whose TITLE happens to end in ": reachable" but carries
        // no tag must not suppress the gate (the old string-match bug).
        let title_only = vec![Finding::ok(ID, "openai: reachable", "ping ok")];
        assert!(total_outage(1, &title_only));
    }
}
