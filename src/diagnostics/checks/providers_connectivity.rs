//! `providers/connectivity` — live LLM provider reachability.
//!
//! Runtime sibling of the path-based core checks. The `aleph doctor` CLI
//! already probes providers through the `providers.healthcheck` RPC, but the
//! LLM-facing `doctor` tool could not — so after the `[f]` AI-repair flow
//! fixed a provider credential (via `vault_store` / `self_config`), the agent
//! had no way to verify the provider was actually reachable again. This check
//! closes that loop: it reuses the same probe as the RPC handler
//! ([`crate::providers::probe::probe_provider`] — one source of truth) against
//! the live daemon config + vault handles.
//!
//! Read-only and **non-repairable**: an unreachable provider is a credential
//! or network condition; the fix is an LLM/user action (store a fresh key,
//! correct the provider section), never a mechanical doctor repair. Findings
//! carry a `fix_hint` routing to those tools.
//!
//! Registered only via [`DiagnosticEngine::with_runtime_checks`]
//! (super::super) — the offline `default_registry()` stays path-only so
//! `aleph-server doctor` keeps working without network access.

use std::time::Duration;

use async_trait::async_trait;
use futures::future::join_all;
use tokio::sync::RwLock;

use crate::config::{Config, ProviderConfig};
use crate::diagnostics::check::{HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, Severity};
use crate::gateway::security::SharedTokenManager;
use crate::providers::probe::{probe_provider, provider_vault_key};
use crate::sync_primitives::Arc;

const ID: &str = "providers/connectivity";

/// Per-provider probe deadline. A hung endpoint must not stall the whole
/// doctor run (the engine awaits all checks).
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

pub struct ProvidersConnectivityCheck {
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
}

impl ProvidersConnectivityCheck {
    pub fn new(config: Arc<RwLock<Config>>, vault: Arc<SharedTokenManager>) -> Self {
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
                match tokio::time::timeout(PROBE_TIMEOUT, probe_provider(&name, runtime)).await {
                    Ok(outcome) if outcome.success => {
                        let lat = outcome
                            .latency_ms
                            .map(|m| format!(" ({m}ms)"))
                            .unwrap_or_default();
                        Finding::ok(ID, format!("{name}: reachable"), format!("ping ok{lat}"))
                    }
                    Ok(outcome) => Finding::problem(
                        ID,
                        Severity::Warning,
                        format!("{name}: unreachable"),
                        outcome
                            .error
                            .unwrap_or_else(|| "unknown probe error".to_string()),
                    )
                    .with_fix_hint(unreachable_hint(&name)),
                    Err(_) => Finding::problem(
                        ID,
                        Severity::Warning,
                        format!("{name}: probe timed out"),
                        format!("no response within {}s", PROBE_TIMEOUT.as_secs()),
                    )
                    .with_fix_hint(unreachable_hint(&name)),
                }
            });

        let mut findings = join_all(futures).await;
        findings.sort_by(|a, b| a.title.cmp(&b.title));
        findings
    }
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
}
