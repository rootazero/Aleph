// src/config/types/phase6_wiring.rs
//! Phase-6 wiring schema — three top-level toml sections that flip
//! Stage 5a/5b + P0 rescue from None placeholders to live values.
//!
//! Missing section → corresponding `Config` field stays `None` →
//! AgentHarnessRunner field stays `None` → behavior identical to
//! Stage 7 ship (commit c2cd8d293) main HEAD.
//!
//! Wired into `AgentHarnessRunner` by `orchestrator_init::build_*` helpers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `[guardrails]` — single switch wiring `PiiSecretsGuardrail::from_globals()`
/// onto Input + Output + ToolCall trait surfaces. Phase-6 has only one real
/// `GuardrailImpl`; future detectors (e.g. content_safety) extend this struct
/// additively without breaking existing toml.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct GuardrailsToml {
    #[serde(default)]
    pub enabled: bool,
}

/// `[stability]` — P0 rescue knobs (stall watchdog + failure cap + per-turn
/// timeout). Each field is `Option<u64>` so callers can opt into a subset;
/// missing fields stay None. `stall_timeout_secs` is the trigger that builds
/// `StallConfig`; `stall_check_interval_secs` falls back to
/// `StallConfig::default().check_interval` (30s) when the timeout is set.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct StabilityToml {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_check_interval_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consecutive_failure_cap: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_timeout_secs: Option<u64>,
}

/// `[fallback_provider]` — Stage 5b single-step fallback. References an
/// existing `[providers.<provider>]` entry by toml key; `ProviderConfig`
/// is *not* inlined here. Self-reference (provider == primary toml key)
/// is detected at build time and yields `None` with a warn log.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct FallbackProviderToml {
    pub provider: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_toml_yields_none_for_three_sections() {
        // Phase-6 acceptance #2: missing section → None for the matching
        // AgentHarnessRunner field. We assert at the schema level here:
        // an empty toml string deserializes the three Option<XxxToml>
        // fields on Config to None.
        #[derive(Deserialize)]
        struct Probe {
            #[serde(default)]
            guardrails: Option<GuardrailsToml>,
            #[serde(default)]
            stability: Option<StabilityToml>,
            #[serde(default)]
            fallback_provider: Option<FallbackProviderToml>,
        }
        let p: Probe = toml::from_str("").expect("empty toml parses");
        assert!(p.guardrails.is_none());
        assert!(p.stability.is_none());
        assert!(p.fallback_provider.is_none());
    }

    #[test]
    fn full_toml_yields_three_some() {
        let toml_str = r#"
[guardrails]
enabled = true

[stability]
stall_timeout_secs = 300
stall_check_interval_secs = 30
consecutive_failure_cap = 8
turn_timeout_secs = 300

[fallback_provider]
provider = "openai-mini"
"#;
        #[derive(Deserialize)]
        struct Probe {
            #[serde(default)]
            guardrails: Option<GuardrailsToml>,
            #[serde(default)]
            stability: Option<StabilityToml>,
            #[serde(default)]
            fallback_provider: Option<FallbackProviderToml>,
        }
        let p: Probe = toml::from_str(toml_str).expect("toml parses");
        assert_eq!(p.guardrails, Some(GuardrailsToml { enabled: true }));
        assert_eq!(
            p.stability,
            Some(StabilityToml {
                stall_timeout_secs: Some(300),
                stall_check_interval_secs: Some(30),
                consecutive_failure_cap: Some(8),
                turn_timeout_secs: Some(300),
            })
        );
        assert_eq!(
            p.fallback_provider,
            Some(FallbackProviderToml {
                provider: "openai-mini".to_string()
            })
        );
    }
}
