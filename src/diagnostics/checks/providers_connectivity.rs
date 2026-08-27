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
//! against the live daemon config + vault handles, and the same derivation of
//! *whether* to dial at all ([`crate::providers::probe::probe_disposition`]).
//! Sharing the probe but not that predicate is what let the two faces disagree:
//! this check honoured the preset's `supports_health_check` opt-out, the RPC did
//! not, and the six opt-out presets came back from one surface as unreachable.
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
use crate::diagnostics::check::{settle_probe, unknown_finding, HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, Severity};
use crate::gateway::security::SharedTokenManager;
use crate::providers::probe::{
    probe_disposition, probe_provider_bounded, provider_vault_key, ProbeDisposition, PROBE_TIMEOUT,
};
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
}

/// What the vault was able to say about one provider's API key.
///
/// Three answers, not two. `Option<String>` can hold "there is a key" and
/// "there is no key" but not "the vault would not tell me", and folding the
/// third into the second is not a quiet degradation here — it decides what
/// this check *reports*. A provider whose stored key could not be read is
/// dialled without it, answers 401, and is published as
/// `{name}: unreachable` with a fix hint telling the operator to store a
/// fresh key with `vault_store`: a confident wrong diagnosis about a
/// credential this check never read, pointing at the one repair that cannot
/// help. Same class as the `[ok]` conflations in `stale_lock` /
/// `duplicate_instance`, one step milder — the wrong *problem* rather than a
/// reassuring pass.
enum KeyLookup {
    /// The vault answered. `None` means no secret is stored for this provider
    /// — a determinate answer, and the ordinary case for OAuth presets.
    Ready(Option<String>),
    /// The vault did not answer; the payload is why, already in the
    /// [`unknown_finding`] house style's `detail` voice.
    Unreadable(String),
}

/// Resolve a provider's API key from the vault (`ai:<name>`). Mirrors the
/// gateway handler's resolution so both surfaces probe identical configs.
///
/// Blocking (vault decryption is synchronous crypto) — callers must keep it
/// off the async executor (`spawn_blocking`).
fn resolve_key(vault: &SharedTokenManager, name: &str) -> KeyLookup {
    match vault.get_secret(&provider_vault_key(name)) {
        Ok(Some(secret)) => KeyLookup::Ready(Some(secret.expose().to_string())),
        Ok(None) => KeyLookup::Ready(None),
        Err(e) => {
            tracing::warn!(provider = %name, error = %e, "vault read failed during connectivity probe");
            KeyLookup::Unreadable(format!(
                "the vault would not release `{}`: {e}",
                provider_vault_key(name)
            ))
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

    /// Derived from the inner bound rather than guessed: every probe is
    /// already capped at [`PROBE_TIMEOUT`] and they run concurrently, so the
    /// check's own honest ceiling is one probe plus scheduling slack. Stating
    /// it here keeps the engine's deadline a backstop for "the bound broke"
    /// instead of a second, unrelated number that could silently become the
    /// tighter one.
    fn timeout(&self) -> std::time::Duration {
        PROBE_TIMEOUT + std::time::Duration::from_secs(5)
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        // Snapshot configs under the read lock, then release it before any
        // blocking or network work (same discipline as `providers.healthcheck`).
        // Key resolution happens AFTER the guard drops: vault decryption is
        // synchronous crypto and must neither run while holding the config
        // lock nor block the async executor.
        let probes: Vec<(String, bool, ProviderConfig)> = {
            let cfg = self.config.read().await;
            cfg.providers
                .iter()
                .map(|(name, pc)| (name.clone(), pc.enabled, pc.clone()))
                .collect()
        };

        if probes.is_empty() {
            return vec![Finding::ok(
                ID,
                "No providers configured",
                "No LLM providers in config.toml; nothing to probe.",
            )];
        }

        let names: Vec<String> = probes.iter().map(|(name, ..)| name.clone()).collect();
        let vault = Arc::clone(&self.vault);
        let keys: Vec<KeyLookup> = match settle_probe(
            ID,
            "Provider credentials",
            tokio::task::spawn_blocking(move || {
                names
                    .iter()
                    .map(|name| resolve_key(&vault, name))
                    .collect::<Vec<_>>()
            })
            .await,
        ) {
            Ok(keys) => keys,
            // A task that never came back read nobody's key. The previous
            // `vec![None; len]` said "none of these providers has a key",
            // which is an answer, and the check then published it as a
            // fleet-wide outage. Every provider inherits the unknown instead;
            // `settle_probe` owns the wording.
            Err(finding) => {
                let why = finding.detail.clone();
                probes
                    .iter()
                    .map(|_| KeyLookup::Unreadable(why.clone()))
                    .collect()
            }
        };
        let probes: Vec<(String, bool, ProviderConfig, KeyLookup)> = probes
            .into_iter()
            .zip(keys)
            .map(|((name, enabled, mut runtime), key)| {
                if let KeyLookup::Ready(k) = &key {
                    runtime.api_key = k.clone();
                }
                (name, enabled, runtime, key)
            })
            .collect();

        // Count providers that will actually be probed (enabled AND whose
        // preset opts in to `/models` health probing) up-front — `probes` is
        // consumed below. The post-join total-outage gate keys off this so a
        // fleet of OAuth-only / placeholder providers that all SKIP probing is
        // never mistaken for a total outage.
        // A provider whose key could not be read is NOT probed below, so it
        // must not be counted here either: leaving it in would make the gate
        // demand a `provider-reachable` tag from a provider nobody dialled,
        // and one unreadable vault would publish "No providers reachable".
        let probed_count = probes
            .iter()
            .filter(|(name, enabled, _, key)| will_be_probed(name, *enabled, key))
            .count();
        let futures = probes
            .into_iter()
            .map(|(name, enabled, runtime, key)| async move {
                // Both faces of "should this provider be dialled" read the same
                // derivation (`probe::probe_disposition`); the `providers.healthcheck`
                // RPC used to answer it with `enabled` alone and reported the
                // opt-out presets as unreachable.
                match probe_disposition(&name, enabled) {
                    ProbeDisposition::Disabled => {
                        return Finding::ok(
                            ID,
                            format!("{name}: disabled"),
                            "Provider is disabled; not probed.",
                        )
                    }
                    // OAuth-only or placeholder-URL providers 404 / rate-limit
                    // on `/models`, so probing them yields a false
                    // `unreachable`. Report `skipped` (Info) instead — and the
                    // outage gate ignores them.
                    ProbeDisposition::Unsupported => {
                        return Finding::ok(
                            ID,
                            format!("{name}: probe skipped"),
                            "Preset opts out of /models health probing (OAuth-only or \
                             placeholder endpoint); credentials not verifiable here.",
                        )
                    }
                    ProbeDisposition::Probe => {}
                }
                // After the two short-circuits, which need no key: reporting
                // "credentials unknown" about a disabled provider would be
                // noise about a provider nobody is going to use.
                if let KeyLookup::Unreadable(why) = key {
                    return credentials_unknown(&name, why);
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
                    // headers, keyed URLs). Redaction is NOT done here: the
                    // engine masks every field of every finding on the way out
                    // (`DiagnosticEngine::run_with_filter` → `Finding::redacted`),
                    // which is the only place that also covers the checks whose
                    // authors never considered credentials. A second call here
                    // would be harmless but would re-teach the wrong lesson —
                    // that each check is responsible for its own masking.
                    Finding::problem(
                        ID,
                        Severity::Warning,
                        format!("{name}: unreachable"),
                        outcome
                            .error
                            .unwrap_or_else(|| "unknown probe error".to_string()),
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

/// Will this provider actually be dialled?
///
/// One predicate with two readers — the outage gate's `probed_count` and the
/// per-provider future — because the gate demands a `provider-reachable` tag
/// from every provider it counted. A provider counted here but short-circuited
/// there is a provider nobody dialled and nobody can vouch for, so a single
/// unreadable vault would publish `No providers reachable`.
fn will_be_probed(name: &str, enabled: bool, key: &KeyLookup) -> bool {
    matches!(key, KeyLookup::Ready(_))
        && probe_disposition(name, enabled) == ProbeDisposition::Probe
}

/// The finding for a provider whose stored credential could not be read.
///
/// Deliberately NOT [`unreachable_hint`]: that hint routes to `vault_store`,
/// which is the one repair that cannot help when the vault itself is what
/// would not answer.
fn credentials_unknown(name: &str, why: String) -> Finding {
    unknown_finding(ID, &format!("{name} credentials"), why).with_fix_hint(format!(
        "Check the vault before trusting any verdict about `{name}`: run doctor's \
         vault check, then re-run this one. Storing a fresh key with vault_store will \
         not help if the vault itself is unreadable."
    ))
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

    /// A credential the vault would not release is not "this provider has no
    /// key". The old `Option<String>` said it was, and the check went on to
    /// dial the provider without it and publish `{name}: unreachable` — a
    /// confident wrong diagnosis, with a fix hint pointing at `vault_store`,
    /// the one repair that cannot help.
    #[test]
    fn an_unreadable_credential_is_not_an_absent_one() {
        let f = credentials_unknown("openai", "the vault would not release `ai:openai`".into());
        assert_eq!(f.severity, Severity::Warning);
        assert_eq!(f.title, "openai credentials unknown");
        assert!(f.is_problem(), "an unknown must never render as [ok]");
        let hint = f.fix_hint.expect("an unknown must say what to look at");
        assert!(hint.contains("vault check"));
        assert!(
            hint.contains("will not help"),
            "the hint must not send the operator to vault_store, which cannot fix an \
             unreadable vault: {hint}"
        );
    }

    /// The outage gate counts providers it will demand an answer from, so the
    /// count and the dial decision must be the same predicate. Stated
    /// name-independently: the key axis vetoes unconditionally, and on the
    /// readable side the answer is exactly `probe_disposition`'s.
    #[test]
    fn a_provider_whose_key_could_not_be_read_is_not_counted_as_probed() {
        for name in ["openai", "anthropic", "not-a-preset"] {
            for enabled in [true, false] {
                assert!(
                    !will_be_probed(name, enabled, &KeyLookup::Unreadable("why".into())),
                    "{name}/{enabled}: an unreadable key must never be counted as probed"
                );
                for key in [KeyLookup::Ready(None), KeyLookup::Ready(Some("k".into()))] {
                    assert_eq!(
                        will_be_probed(name, enabled, &key),
                        probe_disposition(name, enabled) == ProbeDisposition::Probe,
                        "{name}/{enabled}: a readable key must not change the disposition"
                    );
                }
            }
        }
    }
}
