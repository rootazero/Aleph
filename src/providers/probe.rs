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

use crate::config::ProviderConfig;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;

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
