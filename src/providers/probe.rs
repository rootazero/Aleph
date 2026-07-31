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
}
