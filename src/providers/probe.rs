//! Lightweight provider connectivity probe — single source of truth.
//!
//! "Can this provider answer a ping?" is asked from two places: the gateway's
//! `providers.test` / `providers.healthcheck` RPC handlers (Panel surface) and
//! the diagnostics engine's `providers/connectivity` health check (the LLM
//! `doctor` tool). Both call this one function so the probe semantics
//! (create provider → one-shot ping → latency) never drift between surfaces.
//!
//! Lives in the providers domain (not the gateway) so the dependency
//! direction stays `gateway → providers` / `diagnostics → providers` (P1) —
//! core modules never reach into gateway handler internals.

use std::time::Duration;

use crate::config::ProviderConfig;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;

/// Per-provider probe deadline, shared by every probe call site (the
/// `providers.healthcheck` / `providers.test` RPC handlers, the config
/// patcher's post-patch verification, and the `providers/connectivity`
/// doctor check). A hung endpoint must not stall any of them.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Result of one connectivity probe. The gateway maps this onto its
/// wire-stable `TestResult`; diagnostics folds it into `Finding`s.
#[derive(Debug, Clone)]
pub struct ProbeOutcome {
    pub success: bool,
    pub error: Option<String>,
    pub latency_ms: Option<u64>,
}

/// Vault key under which a provider's API key is stored (`ai:<name>`).
/// Shared by the gateway handlers and the diagnostics connectivity check so
/// the key scheme has exactly one definition.
#[must_use]
pub fn provider_vault_key(provider_name: &str) -> String {
    format!("ai:{provider_name}")
}

/// Whether a connectivity sweep should dial this provider at all.
///
/// This module already owned "can it answer a ping"; it did not own "should we
/// even ask", and that one step earlier was answered twice with two different
/// answers. `providers/connectivity` honoured the preset's
/// `supports_health_check` opt-out; `providers.healthcheck` did not, so the six
/// presets that opt out — OAuth-only endpoints and per-deployment hosts that
/// 404 or rate-limit a `/models` probe — came back from the RPC as hard
/// `unreachable` failures while the doctor check reported them, correctly, as
/// not applicable. Neither face was reading the other, and both had tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeDisposition {
    /// Dial it.
    Probe,
    /// The operator turned this provider off.
    Disabled,
    /// The preset declares that a `/models` probe cannot answer for it, so a
    /// failure here would say nothing about the credential.
    Unsupported,
}

/// Decide [`ProbeDisposition`] for one configured provider.
///
/// A provider with no preset (a custom relay) is probeable: the opt-out is a
/// statement a preset makes about its own endpoint, and absence of a preset is
/// not that statement.
#[must_use]
pub fn probe_disposition(provider_name: &str, enabled: bool) -> ProbeDisposition {
    if !enabled {
        return ProbeDisposition::Disabled;
    }
    if crate::providers::presets::get_preset(provider_name)
        .is_some_and(|p| !p.supports_health_check)
    {
        return ProbeDisposition::Unsupported;
    }
    ProbeDisposition::Probe
}

/// Probe a single provider with a lightweight `ping` round-trip and measure
/// latency. Takes a fully-resolved [`ProviderConfig`] (`api_key` already
/// injected from the vault). Read-only: never mutates config or health state.
pub async fn probe_provider(label: &str, provider_config: ProviderConfig) -> ProbeOutcome {
    let provider = match crate::providers::create_provider(label, provider_config) {
        Ok(p) => p,
        Err(e) => {
            return ProbeOutcome {
                success: false,
                error: Some(format!("Failed to create provider: {e}")),
                latency_ms: None,
            };
        }
    };

    let start = std::time::Instant::now();
    let ping_msgs = [UnifiedMessage::user("ping")];
    match provider
        .process(RequestPayload::new(&ping_msgs).with_system(Some("Reply with 'pong'")))
        .await
    {
        Ok(_) => ProbeOutcome {
            success: true,
            error: None,
            latency_ms: Some(u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)),
        },
        Err(e) => ProbeOutcome {
            success: false,
            error: Some(format!("{e}")),
            latency_ms: Some(u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)),
        },
    }
}

/// [`probe_provider`] wrapped in the shared [`PROBE_TIMEOUT`] deadline.
///
/// A timeout is reported as an ordinary failed probe (clear `error`, no
/// latency) so callers need no `tokio::time::timeout` boilerplate of their
/// own — every surface should call this variant, not the raw probe.
pub async fn probe_provider_bounded(label: &str, provider_config: ProviderConfig) -> ProbeOutcome {
    match tokio::time::timeout(PROBE_TIMEOUT, probe_provider(label, provider_config)).await {
        Ok(outcome) => outcome,
        Err(_) => ProbeOutcome {
            success: false,
            error: Some(format!(
                "probe timed out after {}s",
                PROBE_TIMEOUT.as_secs()
            )),
            latency_ms: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bounded_probe_reports_provider_creation_failure() {
        // An unknown provider label fails inside `create_provider` — long
        // before any network I/O — so this exercises the bounded wrapper's
        // pass-through path without depending on the network.
        let outcome = probe_provider_bounded(
            "no-such-provider",
            ProviderConfig::test_config("test-model"),
        )
        .await;
        assert!(!outcome.success);
        assert!(outcome.error.is_some());
    }

    #[test]
    fn shared_timeout_is_ten_seconds() {
        assert_eq!(PROBE_TIMEOUT, Duration::from_secs(10));
    }

    #[test]
    fn the_operator_switch_outranks_everything() {
        assert_eq!(
            probe_disposition("openai", false),
            ProbeDisposition::Disabled
        );
        // Even for a preset that could not be probed anyway: the operator's
        // own answer is the one to report back.
        assert_eq!(
            probe_disposition("chatgpt", false),
            ProbeDisposition::Disabled
        );
    }

    /// The bit that used to be read on one face and not the other.
    #[test]
    fn a_preset_that_opts_out_of_probing_is_not_dialled() {
        for name in crate::providers::presets::canonical_profiles()
            .iter()
            .filter(|(_, p)| !p.supports_health_check)
            .map(|(name, _)| *name)
        {
            assert_eq!(
                probe_disposition(name, true),
                ProbeDisposition::Unsupported,
                "{name} declares no /models endpoint, so a probe can only fail"
            );
        }
    }

    #[test]
    fn a_provider_with_no_preset_is_probeable() {
        // The opt-out is a statement a preset makes about its own endpoint.
        // Absence of a preset is not that statement, and a custom relay is
        // OpenAI-compatible by construction.
        assert_eq!(
            probe_disposition("some-custom-relay", true),
            ProbeDisposition::Probe
        );
        assert_eq!(probe_disposition("openai", true), ProbeDisposition::Probe);
    }
}
