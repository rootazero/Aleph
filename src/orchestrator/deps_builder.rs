//! Shared HarnessDeps builder functions.
//!
//! Used by both the main runner (`aleph-server` bin's `orchestrator_init.rs`)
//! and the subagent spawner (`agents::subagent_spawner`) to assemble
//! HarnessDeps fields consistently. Subagents inherit identical config; no
//! override params are accepted (per P1 zero-override decision).

use std::sync::Arc;
use std::time::Duration;

use crate::config::Config;
use crate::harness::StallConfig;
use crate::providers::AiProvider;

/// Stability triple — three independent Optionals derived from `[stability]`.
///
/// Returned as a struct (not tuple) so consumers can name fields and future
/// additions don't break callers.
pub struct StabilityTriple {
    pub stall_config: Option<StallConfig>,
    pub consecutive_failure_cap: Option<usize>,
    pub turn_timeout: Option<Duration>,
}

/// Build the optional Stage 5b single-step fallback provider from
/// `[fallback_provider]`. Returns `None` if:
/// - section missing
/// - `provider` matches `primary_provider_key` ASCII-case-insensitively
/// - `provider` not present in `[providers]` map (warn log)
/// - `create_provider` failure (warn log; e.g. unknown protocol)
pub fn build_fallback_llm(
    config: &Config,
    primary_provider_key: &str,
) -> Option<Arc<dyn AiProvider>> {
    let fb = config.fallback_provider.as_ref()?;
    if fb.provider.eq_ignore_ascii_case(primary_provider_key) {
        tracing::warn!(
            provider = %fb.provider,
            "fallback_provider self-reference; disabling"
        );
        return None;
    }
    let pc = match config.providers.get(&fb.provider) {
        Some(c) => c.clone(),
        None => {
            tracing::warn!(
                provider = %fb.provider,
                "fallback_provider not found in [providers]; disabling"
            );
            return None;
        }
    };
    match crate::providers::create_provider(&fb.provider, pc) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(
                provider = %fb.provider,
                error = %e,
                "fallback_provider create_provider failed; disabling"
            );
            None
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::{FallbackProviderToml, ProviderConfig, StabilityToml};

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

    #[test]
    fn fallback_returns_none_when_section_missing() {
        let cfg = Config::default();
        assert!(build_fallback_llm(&cfg, "primary").is_none());
    }

    #[test]
    fn fallback_returns_none_on_self_reference() {
        let cfg = cfg_with_fallback(
            Some(FallbackProviderToml {
                provider: "Primary".to_string(),
            }),
            vec![("primary", mock_provider_config())],
        );
        // ASCII case-insensitive match → self-reference detected.
        assert!(build_fallback_llm(&cfg, "primary").is_none());
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
