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

/// `[fallback_provider]` — the ordered provider failover chain.
///
/// Each entry references an existing `[providers.<name>]` entry by its toml
/// key; `ProviderConfig` is *not* inlined. The primary provider is tried
/// first, then each chain provider in order — every provider expanded across
/// its own `models` list. Entries matching the primary, unknown providers,
/// or providers that fail to construct are skipped with a warn log.
///
/// Two equivalent forms are accepted:
///
/// ```toml
/// [fallback_provider]
/// chain = ["openai", "gemini"]
/// ```
///
/// ```toml
/// # back-compat single-provider form (folded into `chain`)
/// [fallback_provider]
/// provider = "openai"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct FallbackProviderToml {
    /// Ordered list of fallback provider toml keys.
    #[serde(default)]
    pub chain: Vec<String>,
    /// Back-compat single-provider form. Folded into the effective chain by
    /// [`FallbackProviderToml::resolved_chain`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

impl FallbackProviderToml {
    /// The effective ordered fallback chain: explicit `chain` entries, then
    /// the back-compat `provider` when set and not already present.
    pub fn resolved_chain(&self) -> Vec<String> {
        let mut out = self.chain.clone();
        if let Some(p) = &self.provider {
            if !out.iter().any(|c| c.eq_ignore_ascii_case(p)) {
                out.push(p.clone());
            }
        }
        out
    }
}

/// `[context_budget]` — opt-in mid-run context-window management. When
/// `enabled = true`, the harness senses context pressure between turns and
/// compacts older conversation history (LLM summarization, with a
/// deterministic-truncation fallback) before the window overflows — so a
/// long Think→Act run does not hard-fail on a provider context-length error.
///
/// Missing section, or `enabled = false`, leaves the feature off: behavior is
/// identical to before this wiring (`context_budget`/`context_compactor` stay
/// `None` on `HarnessDeps`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ContextBudgetToml {
    #[serde(default)]
    pub enabled: bool,
    /// Model context-window size in tokens. Set this to your model's real
    /// window — compaction thresholds are fractions of this budget, so an
    /// inaccurate value compacts too early or too late. Default 200_000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
    /// Fraction of `token_budget` at which compaction begins. Default 0.70.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning_threshold: Option<f64>,
    /// Fraction of `token_budget` at which the run is forced to a final reply
    /// without further tool calls. Default 0.85.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critical_threshold: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Probe {
        #[serde(default)]
        guardrails: Option<GuardrailsToml>,
        #[serde(default)]
        stability: Option<StabilityToml>,
        #[serde(default)]
        fallback_provider: Option<FallbackProviderToml>,
        #[serde(default)]
        context_budget: Option<ContextBudgetToml>,
    }

    #[test]
    fn empty_toml_yields_none_for_all_sections() {
        // Phase-6 acceptance #2: missing section → None for the matching
        // AgentHarnessRunner field. We assert at the schema level here:
        // an empty toml string deserializes the Option<XxxToml> fields on
        // Config to None.
        let p: Probe = toml::from_str("").expect("empty toml parses");
        assert!(p.guardrails.is_none());
        assert!(p.stability.is_none());
        assert!(p.fallback_provider.is_none());
        assert!(p.context_budget.is_none());
    }

    #[test]
    fn full_toml_yields_all_some() {
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

[context_budget]
enabled = true
token_budget = 128000
warning_threshold = 0.7
critical_threshold = 0.85
"#;
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
                chain: Vec::new(),
                provider: Some("openai-mini".to_string()),
            })
        );
        assert_eq!(
            p.context_budget,
            Some(ContextBudgetToml {
                enabled: true,
                token_budget: Some(128_000),
                warning_threshold: Some(0.7),
                critical_threshold: Some(0.85),
            })
        );
    }

    #[test]
    fn fallback_provider_chain_form_parses() {
        let p: Probe = toml::from_str("[fallback_provider]\nchain = [\"openai\", \"gemini\"]\n")
            .expect("toml parses");
        let fb = p.fallback_provider.expect("section present");
        assert_eq!(fb.chain, vec!["openai", "gemini"]);
        assert_eq!(fb.resolved_chain(), vec!["openai", "gemini"]);
    }

    #[test]
    fn fallback_provider_resolved_chain_folds_back_compat_provider() {
        // Legacy single-provider form folds into the effective chain.
        let legacy = FallbackProviderToml {
            chain: Vec::new(),
            provider: Some("openai".to_string()),
        };
        assert_eq!(legacy.resolved_chain(), vec!["openai"]);

        // A `provider` already present in `chain` is not duplicated.
        let both = FallbackProviderToml {
            chain: vec!["openai".to_string(), "gemini".to_string()],
            provider: Some("openai".to_string()),
        };
        assert_eq!(both.resolved_chain(), vec!["openai", "gemini"]);
    }

    #[test]
    fn context_budget_section_defaults_to_disabled() {
        // `[context_budget]` present but `enabled` omitted → enabled = false,
        // so the feature stays off unless explicitly switched on.
        let p: Probe = toml::from_str("[context_budget]\n").expect("toml parses");
        let cb = p.context_budget.expect("section present");
        assert!(!cb.enabled);
        assert!(cb.token_budget.is_none());
    }
}
