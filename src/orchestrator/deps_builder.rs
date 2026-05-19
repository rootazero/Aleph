//! Shared HarnessDeps builder functions.
//!
//! Used by both the main runner (`aleph-server` bin's `orchestrator_init.rs`)
//! and the subagent spawner (`agents::subagent_spawner`) to assemble
//! HarnessDeps fields consistently. Subagents inherit identical config; no
//! override params are accepted (per P1 zero-override decision).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::config::Config;
use crate::context::budget::ContextBudgetConfig;
use crate::harness::StallConfig;
use crate::providers::{
    create_provider, DefaultProviderHandle, FailoverConfig, FailoverHealth, FailoverNode,
    FailoverProvider, StaticDefault,
};

/// Default model context-window estimate (tokens) used when `[context_budget]`
/// omits `token_budget`. Operators on larger- or smaller-window models should
/// set `token_budget` explicitly — compaction thresholds are fractions of it.
const DEFAULT_CONTEXT_TOKEN_BUDGET: u64 = 200_000;

/// Stability triple — three independent Optionals derived from `[stability]`.
///
/// Returned as a struct (not tuple) so consumers can name fields and future
/// additions don't break callers.
pub struct StabilityTriple {
    pub stall_config: Option<StallConfig>,
    pub consecutive_failure_cap: Option<usize>,
    pub turn_timeout: Option<Duration>,
}

/// Wrap `default_provider` in a [`FailoverProvider`] that walks the ordered
/// chain `[primary, ...[fallback_provider].chain]`.
///
/// The primary slot stays *live*: the returned handle's `FailoverProvider`
/// reads `default_provider.current()` on every call, so a UI `set_default`
/// swap still takes effect on the next turn (hot-reload preserved).
///
/// Every configured provider's `models` list feeds the catalog, so the chain
/// can also fail over *between models* of one provider. A chain entry that
/// matches the primary, names a provider absent from `[providers]`, or fails
/// to construct is skipped with a warn log.
///
/// Always returns a `FailoverProvider` handle — even with an empty chain it
/// adds model-level fallback, transient in-place retry, and a per-provider
/// circuit breaker over the primary.
pub fn build_failover_chain(
    config: &Config,
    primary_provider_key: &str,
    default_provider: Arc<dyn DefaultProviderHandle>,
) -> Arc<dyn DefaultProviderHandle> {
    // Catalog of every configured provider's model list, keyed by toml name.
    // Lets the live primary — and each fallback — fail over across models.
    let model_catalog: HashMap<String, Vec<String>> = config
        .providers
        .iter()
        .map(|(name, pc)| (name.clone(), pc.all_models().to_vec()))
        .collect();

    let mut fallbacks: Vec<FailoverNode> = Vec::new();
    if let Some(fb) = config.fallback_provider.as_ref() {
        for name in fb.resolved_chain() {
            if name.eq_ignore_ascii_case(primary_provider_key) {
                tracing::warn!(provider = %name, "failover chain: entry matches primary; skipping");
                continue;
            }
            let pc = match config.providers.get(&name) {
                Some(c) => c.clone(),
                None => {
                    tracing::warn!(
                        provider = %name,
                        "failover chain: provider not in [providers]; skipping"
                    );
                    continue;
                }
            };
            let models = pc.all_models().to_vec();
            match create_provider(&name, pc) {
                Ok(provider) => fallbacks.push(FailoverNode {
                    name,
                    models,
                    provider,
                }),
                Err(e) => tracing::warn!(
                    provider = %name,
                    error = %e,
                    "failover chain: create_provider failed; skipping"
                ),
            }
        }
    }

    tracing::info!(
        primary = %primary_provider_key,
        fallback_count = fallbacks.len(),
        "failover chain assembled"
    );

    let failover = FailoverProvider::new(
        default_provider,
        fallbacks,
        model_catalog,
        FailoverHealth::default(),
        FailoverConfig::default(),
    );
    Arc::new(StaticDefault::new(Arc::new(failover)))
}

/// Build the P0 rescue triple from `[stability]`. Each field is independent.
pub fn build_stability_triple(config: &Config) -> StabilityTriple {
    let Some(s) = config.stability.as_ref() else {
        return StabilityTriple {
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
        };
    };
    let stall_config = s.stall_timeout_secs.map(|secs| {
        let mut sc = StallConfig::default().with_timeout(Duration::from_secs(secs));
        if let Some(ci) = s.stall_check_interval_secs {
            sc = sc.with_check_interval(Duration::from_secs(ci));
        }
        sc
    });
    StabilityTriple {
        stall_config,
        consecutive_failure_cap: s.consecutive_failure_cap,
        turn_timeout: s.turn_timeout_secs.map(Duration::from_secs),
    }
}

/// Build the optional per-run context-budget config from `[context_budget]`.
///
/// Returns `None` when the section is absent or `enabled = false` — the
/// orchestrator then leaves `HarnessDeps.context_budget`/`context_compactor`
/// as `None`, so behavior is identical to before this wiring (no mid-run
/// compaction). When `Some`, `AgentHarnessRunner::run` constructs a *fresh*
/// `ContextBudget` per run (its circuit-breaker / diminishing-returns state
/// must not be shared across concurrent sessions).
///
/// `token_budget` and the two thresholds are user-tunable; the remaining
/// `ContextBudgetConfig` fields use validated internal defaults (KISS — not
/// every knob needs a toml surface).
pub fn build_context_budget_config(config: &Config) -> Option<ContextBudgetConfig> {
    let cb = config.context_budget.as_ref()?;
    if !cb.enabled {
        return None;
    }
    let token_budget = cb.token_budget.unwrap_or(DEFAULT_CONTEXT_TOKEN_BUDGET);
    let warning_threshold = cb.warning_threshold.unwrap_or(0.70);
    let critical_threshold = cb.critical_threshold.unwrap_or(0.85);

    // Defensive validation (P7): a misconfigured budget silently cuts off
    // every run — e.g. inverted thresholds make `CompactAndContinue`
    // unreachable so every warning-pressure turn escalates to `FinalReply`.
    // Reject rather than degrade.
    if token_budget == 0 {
        tracing::warn!("context_budget: token_budget must be > 0; disabling");
        return None;
    }
    if !(warning_threshold > 0.0
        && warning_threshold < critical_threshold
        && critical_threshold <= 1.0)
    {
        tracing::warn!(
            warning_threshold,
            critical_threshold,
            "context_budget: require 0 < warning_threshold < critical_threshold <= 1.0; disabling"
        );
        return None;
    }

    Some(ContextBudgetConfig {
        token_budget,
        warning_threshold,
        critical_threshold,
        // Internal tuning — validated defaults, deliberately not exposed as
        // toml knobs (KISS: every run inherits the same compaction cadence).
        token_estimate_ratio: 3.5,
        fresh_tail_count: 6,
        circuit_breaker_max: 3,
        diminishing_window: 4,
        diminishing_threshold: 500,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::{ContextBudgetToml, FallbackProviderToml, ProviderConfig, StabilityToml};

    fn cfg_with_context_budget(cb: Option<ContextBudgetToml>) -> Config {
        Config {
            context_budget: cb,
            ..Config::default()
        }
    }

    #[test]
    fn context_budget_none_when_section_missing() {
        let cfg = Config::default();
        assert!(build_context_budget_config(&cfg).is_none());
    }

    #[test]
    fn context_budget_none_when_disabled() {
        let cfg = cfg_with_context_budget(Some(ContextBudgetToml {
            enabled: false,
            token_budget: Some(128_000),
            ..ContextBudgetToml::default()
        }));
        assert!(build_context_budget_config(&cfg).is_none());
    }

    #[test]
    fn context_budget_some_uses_defaults_when_fields_unset() {
        let cfg = cfg_with_context_budget(Some(ContextBudgetToml {
            enabled: true,
            ..ContextBudgetToml::default()
        }));
        let bc = build_context_budget_config(&cfg).expect("enabled → Some");
        assert_eq!(bc.token_budget, DEFAULT_CONTEXT_TOKEN_BUDGET);
        assert_eq!(bc.warning_threshold, 0.70);
        assert_eq!(bc.critical_threshold, 0.85);
    }

    #[test]
    fn context_budget_some_honours_explicit_values() {
        let cfg = cfg_with_context_budget(Some(ContextBudgetToml {
            enabled: true,
            token_budget: Some(64_000),
            warning_threshold: Some(0.6),
            critical_threshold: Some(0.9),
        }));
        let bc = build_context_budget_config(&cfg).expect("enabled → Some");
        assert_eq!(bc.token_budget, 64_000);
        assert_eq!(bc.warning_threshold, 0.6);
        assert_eq!(bc.critical_threshold, 0.9);
    }

    #[test]
    fn context_budget_none_when_thresholds_inverted() {
        // warning >= critical makes the CompactAndContinue branch unreachable;
        // every warning-pressure turn escalates straight to FinalReply,
        // silently cutting off every run. Reject rather than degrade.
        let cfg = cfg_with_context_budget(Some(ContextBudgetToml {
            enabled: true,
            warning_threshold: Some(0.9),
            critical_threshold: Some(0.7),
            ..ContextBudgetToml::default()
        }));
        assert!(build_context_budget_config(&cfg).is_none());
    }

    #[test]
    fn context_budget_none_when_token_budget_zero() {
        let cfg = cfg_with_context_budget(Some(ContextBudgetToml {
            enabled: true,
            token_budget: Some(0),
            ..ContextBudgetToml::default()
        }));
        assert!(build_context_budget_config(&cfg).is_none());
    }

    #[test]
    fn context_budget_none_when_threshold_out_of_range() {
        let cfg = cfg_with_context_budget(Some(ContextBudgetToml {
            enabled: true,
            warning_threshold: Some(1.5),
            ..ContextBudgetToml::default()
        }));
        assert!(build_context_budget_config(&cfg).is_none());
    }

    fn cfg_with_fallback(
        fb: Option<FallbackProviderToml>,
        providers: Vec<(&str, ProviderConfig)>,
    ) -> Config {
        let mut providers_map: std::collections::HashMap<String, ProviderConfig> =
            std::collections::HashMap::new();
        for (k, v) in providers {
            providers_map.insert(k.to_string(), v);
        }
        Config {
            fallback_provider: fb,
            providers: providers_map,
            ..Config::default()
        }
    }

    fn mock_provider_config() -> ProviderConfig {
        let mut pc = ProviderConfig::test_config("mock-model");
        pc.protocol = Some("mock".to_string());
        pc
    }

    fn mock_handle(name: &str) -> Arc<dyn DefaultProviderHandle> {
        let provider = create_provider(name, mock_provider_config()).expect("mock provider");
        Arc::new(StaticDefault::new(provider))
    }

    #[test]
    fn failover_chain_wraps_primary_even_without_fallback_section() {
        // No `[fallback_provider]` — the primary is still wrapped so it gains
        // model-level fallback, transient retry, and a circuit breaker.
        let cfg = cfg_with_fallback(None, vec![("primary", mock_provider_config())]);
        let wrapped = build_failover_chain(&cfg, "primary", mock_handle("primary"));
        assert_eq!(wrapped.current().name(), "failover");
    }

    #[test]
    fn failover_chain_builds_with_configured_chain() {
        let cfg = cfg_with_fallback(
            Some(FallbackProviderToml {
                chain: vec!["fb".to_string()],
                provider: None,
            }),
            vec![
                ("primary", mock_provider_config()),
                ("fb", mock_provider_config()),
            ],
        );
        let wrapped = build_failover_chain(&cfg, "primary", mock_handle("primary"));
        assert_eq!(wrapped.current().name(), "failover");
    }

    #[test]
    fn failover_chain_skips_self_reference_and_unknown_providers() {
        // `Primary` (self, case-insensitive) and `ghost` (absent from
        // `[providers]`) are both skipped with a warn log; the build still
        // succeeds and yields a failover handle.
        let cfg = cfg_with_fallback(
            Some(FallbackProviderToml {
                chain: vec!["Primary".to_string(), "ghost".to_string()],
                provider: None,
            }),
            vec![("primary", mock_provider_config())],
        );
        let wrapped = build_failover_chain(&cfg, "primary", mock_handle("primary"));
        assert_eq!(wrapped.current().name(), "failover");
    }

    #[test]
    fn stability_triple_independence_all_none() {
        let cfg = Config::default();
        let triple = build_stability_triple(&cfg);
        assert!(triple.stall_config.is_none());
        assert!(triple.consecutive_failure_cap.is_none());
        assert!(triple.turn_timeout.is_none());
    }

    #[test]
    fn stability_triple_only_turn_timeout_set() {
        let cfg = Config {
            stability: Some(StabilityToml {
                stall_timeout_secs: None,
                stall_check_interval_secs: None,
                consecutive_failure_cap: None,
                turn_timeout_secs: Some(60),
            }),
            ..Config::default()
        };
        let triple = build_stability_triple(&cfg);
        assert!(triple.stall_config.is_none());
        assert!(triple.consecutive_failure_cap.is_none());
        assert_eq!(triple.turn_timeout, Some(Duration::from_secs(60)));
    }
}
