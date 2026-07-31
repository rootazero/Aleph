// src/config/types/phase6_wiring.rs
//! Phase-6 wiring schema — three top-level toml sections that flip
//! Stage 5a/5b + P0 rescue from None placeholders to live values.
//!
//! Missing section → corresponding `Config` field stays `None` →
//! `AgentHarnessRunner` field stays `None` → behavior identical to
//! Stage 7 ship (commit c2cd8d293) main HEAD.
//!
//! Wired into `AgentHarnessRunner` by `orchestrator_init::build_*` helpers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `[guardrails]` — single switch wiring `PiiSecretsGuardrail`
/// onto Input + Output + `ToolCall` trait surfaces. Phase-6 has only one real
/// `GuardrailImpl`; future detectors (e.g. `content_safety`) extend this struct
/// additively without breaking existing toml.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct GuardrailsToml {
    #[serde(default)]
    pub enabled: bool,
}

/// `[stability]` — P0 rescue knobs (stall watchdog + failure cap + per-turn
/// timeout). Each field is `Option<u64>` so callers can opt into a subset;
/// missing fields stay None. `stall_timeout_secs` is the trigger that builds
/// `StallConfig`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct StabilityToml {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_timeout_secs: Option<u64>,
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
    /// Same-candidate retries on a transient error before the chain advances.
    /// `None` keeps the built-in default (2). Raise it for single-provider
    /// setups that have no sibling to fail over to, so a stubborn transient
    /// throttle is ridden out longer in place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
}

impl FallbackProviderToml {
    /// The effective ordered fallback chain: explicit `chain` entries, then
    /// the back-compat `provider` when set and not already present.
    #[must_use]
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
    /// inaccurate value compacts too early or too late. Default `200_000`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
    /// Fraction of `token_budget` at which compaction begins. When unset the
    /// default is **window-aware**: a wide window (≈1M) resolves to `0.70`,
    /// while a narrow window (≈200k) automatically compacts earlier so one
    /// large tool result cannot leap the warning→critical band and overflow
    /// before compaction fires. Set this to pin a fixed fraction regardless of
    /// window size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning_threshold: Option<f64>,
    /// Fraction of `token_budget` at which the run is forced to a final reply
    /// without further tool calls. Default 0.85.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critical_threshold: Option<f64>,
    /// Cheap-tier model id for side-channel history summarization (Reasonix
    /// `summaryModel` parity). When set, compaction's summarization call routes
    /// to a provider built from the *primary* provider's config (same vendor /
    /// API key / endpoint / protocol) with this model substituted — typically a
    /// flash-tier sibling (e.g. `claude-haiku-4-5`, `gemini-3-flash-preview`,
    /// `deepseek-v4-flash`). Summarization is read-and-condense work where the
    /// strongest model is almost never required, so routing it here yields a
    /// large per-token cost reduction with no measurable quality regression.
    ///
    /// **Unset / empty (default):** auto-fall back to the primary provider
    /// preset's declared cheap aux model (`default_aux_model` — openai→
    /// `gpt-5.4-mini`, claude→`claude-haiku-4-5`, gemini→`gemini-3-flash-preview`,
    /// deepseek→`deepseek-v4-flash`). A provider whose preset declares no cheap tier
    /// (or a custom/unknown provider key) keeps summarization on the main LLM.
    ///
    /// **Opt out:** set this to the primary's own model id — the resolved model
    /// then equals the configured default, so no separate provider is built and
    /// summarization stays on the main LLM. A model id that fails to build a
    /// provider (bad protocol/preset) also degrades silently to the main LLM —
    /// never a hard boot failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_model: Option<String>,
    /// Per-model trigger-point overrides. The compaction budget is sized for
    /// the *smallest* context window on the failover chain, so the warning /
    /// critical fractions are keyed off that same model. These let a narrow
    /// model compact more aggressively than a wide one — e.g. a tighter
    /// `0.60 / 0.78` for a 200k-window model while a 1M-window model keeps the
    /// default `0.70 / 0.85`. The first entry whose `model` matches the
    /// resolved model id (or the provider key) wins; unset fields fall back to
    /// the top-level thresholds, then the built-in `0.70 / 0.85`. Empty by
    /// default (every model inherits the global thresholds).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_thresholds: Vec<ModelThresholdToml>,
    /// Verbatim tail budget, in tokens, for **user-driven** `/compact` (pi's
    /// `keepRecentTokens`; its default is 20 000 too). Everything older than
    /// this is summarized into a single `[Context Summary]` and stops being
    /// replayed; the tail is kept word-for-word.
    ///
    /// A token budget rather than a message count on purpose: the retired
    /// `KeepLastN{50}` rule was wrong in both directions — fifty `file_read`
    /// results can be 200k tokens, and fifty one-line turns are a few hundred.
    ///
    /// Unset uses `manual::DEFAULT_KEEP_TOKENS`. Values outside
    /// `[2_000, 400_000]` are clamped: below the floor a compaction would eat
    /// the turn that asked for it, above the ceiling the command would be a
    /// silent no-op. Independent of the automatic in-turn compaction above,
    /// which is pressure-driven and never touches the stored log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_compact_keep_tokens: Option<usize>,
}

/// One `[[context_budget.model_thresholds]]` entry: a per-model override of the
/// compaction trigger fractions, selected by a case-insensitive substring match
/// against the resolved model id or provider key.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ModelThresholdToml {
    /// Case-insensitive substring matched against the resolved model id and the
    /// provider key (e.g. `"kimi"`, `"claude"`, `"moonshot"`). The first
    /// matching entry in declaration order wins.
    pub model: String,
    /// Fraction of `token_budget` at which compaction begins for this model.
    /// Unset → inherit the top-level `warning_threshold` (then `0.70`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning_threshold: Option<f64>,
    /// Fraction of `token_budget` at which this model is forced to a final
    /// reply. Unset → inherit the top-level `critical_threshold` (then `0.85`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critical_threshold: Option<f64>,
}

impl ContextBudgetToml {
    /// First per-model threshold override matching the resolved `model` id or
    /// `provider` key (case-insensitive substring). Returns `None` when the
    /// list is empty or nothing matches — callers then use the global
    /// thresholds, so behaviour is byte-identical without any override.
    #[must_use]
    pub fn threshold_override_for(
        &self,
        model: Option<&str>,
        provider: &str,
    ) -> Option<&ModelThresholdToml> {
        let provider_lc = provider.to_lowercase();
        let model_lc = model.map(str::to_lowercase);
        self.model_thresholds.iter().find(|o| {
            let needle = o.model.trim().to_lowercase();
            if needle.is_empty() {
                return false;
            }
            model_lc.as_deref().is_some_and(|m| m.contains(&needle))
                || provider_lc.contains(&needle)
        })
    }
}

/// `[strategy]` — opt-in (default **on**) strategic-planner welding for the
/// three long-task entry points (`/goal`, `/loop`, `/workflow`). When enabled,
/// a one-shot tool-free LLM planning node mints a short `Strategy` that is
/// welded into the cacheable system-prompt prefix of every downstream turn, so
/// a long task stays anchored to its objective and a small set of concrete
/// anti-distraction guardrails. The planner fires **once per task** (above the
/// Think→Act loop), is fully fail-soft, and self-gates: a trivial task or any
/// failure stores no Strategy and leaves the prompt byte-identical.
///
/// Unlike `[context_budget]`, this section defaults to **enabled = true**: the
/// welded Strategy is a model-independent context-engineering win (KV-cache
/// prefix reuse + attention anchoring), so it ships on. `enabled = false` is the
/// one-flip A/B + Future-Proof escape valve (spec §2): if a future model shows
/// zero goal-drift on these flows, the feature retires via config, no code
/// change.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct StrategyToml {
    /// Master switch. Default **true** (on). Set `false` to fully disable the
    /// planner — no extra LLM call, no welded prefix, byte-identical prompts.
    #[serde(default = "strategy_enabled_default")]
    pub enabled: bool,
    /// Optional planner model id. When set, the planning call routes to a
    /// provider built from the *primary* provider's config (same vendor / API
    /// key / endpoint / protocol) with this model substituted. Unset (default)
    /// ⇒ the planner reuses the executor's main provider — planning is
    /// reasoning-heavy, so unlike `[context_budget].summary_model` there is no
    /// cheap-tier auto-fallback (that would silently downgrade the strategist).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner_model: Option<String>,
    /// Whether the strategic planner also fires for a NAKED agent-loop first
    /// message (a plain complex request that did NOT use /goal·/loop·/workflow).
    /// Default **true** (gated under `enabled`). Set `false` to keep planning
    /// only on the three explicit long-task flows and spare ordinary chat the
    /// turn-1 planner latency. The planner still self-gates trivial messages.
    #[serde(default = "strategy_plan_naked_loop_default")]
    pub plan_naked_loop: bool,
    /// Whether the strategic planner also fires for a TEAM group-chat first
    /// message (strategy round 2 → teams). Default **true** (gated under
    /// `enabled`). Set `false` to keep team group chat exactly as before — no
    /// leader-first planning, no welded team strategy, byte-identical prompts.
    #[serde(default = "strategy_plan_team_default")]
    pub plan_team: bool,
}

/// serde default for `StrategyToml::enabled` — the planner is on unless an
/// operator explicitly flips it off.
fn strategy_enabled_default() -> bool {
    true
}

/// serde default for `StrategyToml::plan_naked_loop` — naked-loop planning is
/// on unless an operator explicitly flips it off.
fn strategy_plan_naked_loop_default() -> bool {
    true
}

/// serde default for `StrategyToml::plan_team` — team planning is on unless an
/// operator explicitly flips it off.
fn strategy_plan_team_default() -> bool {
    true
}

impl Default for StrategyToml {
    fn default() -> Self {
        Self {
            enabled: strategy_enabled_default(),
            planner_model: None,
            plan_naked_loop: strategy_plan_naked_loop_default(),
            plan_team: strategy_plan_team_default(),
        }
    }
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
        #[serde(default)]
        strategy: Option<StrategyToml>,
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
        assert!(p.strategy.is_none());
    }

    #[test]
    fn full_toml_yields_all_some() {
        let toml_str = r#"
[guardrails]
enabled = true

[stability]
stall_timeout_secs = 300
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
                consecutive_failure_cap: Some(8),
                turn_timeout_secs: Some(300),
            })
        );
        assert_eq!(
            p.fallback_provider,
            Some(FallbackProviderToml {
                chain: Vec::new(),
                provider: Some("openai-mini".to_string()),
                max_retries: None,
            })
        );
        assert_eq!(
            p.context_budget,
            Some(ContextBudgetToml {
                enabled: true,
                token_budget: Some(128_000),
                warning_threshold: Some(0.7),
                critical_threshold: Some(0.85),
                summary_model: None,
                model_thresholds: Vec::new(),
                manual_compact_keep_tokens: None,
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
        // Unset `max_retries` keeps the built-in default downstream.
        assert_eq!(fb.max_retries, None);
    }

    #[test]
    fn fallback_provider_parses_max_retries() {
        let p: Probe =
            toml::from_str("[fallback_provider]\nchain = [\"openai\"]\nmax_retries = 6\n")
                .expect("toml parses");
        let fb = p.fallback_provider.expect("section present");
        assert_eq!(fb.max_retries, Some(6));
    }

    #[test]
    fn fallback_provider_resolved_chain_folds_back_compat_provider() {
        // Legacy single-provider form folds into the effective chain.
        let legacy = FallbackProviderToml {
            chain: Vec::new(),
            provider: Some("openai".to_string()),
            max_retries: None,
        };
        assert_eq!(legacy.resolved_chain(), vec!["openai"]);

        // A `provider` already present in `chain` is not duplicated.
        let both = FallbackProviderToml {
            chain: vec!["openai".to_string(), "gemini".to_string()],
            provider: Some("openai".to_string()),
            max_retries: None,
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
        // Per-model overrides default to empty → every model inherits globals.
        assert!(cb.model_thresholds.is_empty());
        // Cheap summarization is opt-in — unset by default.
        assert!(cb.summary_model.is_none());
    }

    #[test]
    fn summary_model_parses_for_cheap_tier_summarization() {
        let toml_str = "[context_budget]\nenabled = true\nsummary_model = \"claude-haiku-4-5\"\n";
        let p: Probe = toml::from_str(toml_str).expect("toml parses");
        let cb = p.context_budget.expect("section present");
        assert_eq!(cb.summary_model.as_deref(), Some("claude-haiku-4-5"));
    }

    #[test]
    fn model_thresholds_parse_as_array_of_tables() {
        let toml_str = r#"
[context_budget]
enabled = true
warning_threshold = 0.70
critical_threshold = 0.85

[[context_budget.model_thresholds]]
model = "kimi"
warning_threshold = 0.60
critical_threshold = 0.78

[[context_budget.model_thresholds]]
model = "claude"
"#;
        let p: Probe = toml::from_str(toml_str).expect("toml parses");
        let cb = p.context_budget.expect("section present");
        assert_eq!(cb.model_thresholds.len(), 2);
        assert_eq!(cb.model_thresholds[0].model, "kimi");
        assert_eq!(cb.model_thresholds[0].warning_threshold, Some(0.60));
        assert_eq!(cb.model_thresholds[0].critical_threshold, Some(0.78));
        // Second entry overrides nothing — a placeholder that inherits globals.
        assert_eq!(cb.model_thresholds[1].model, "claude");
        assert_eq!(cb.model_thresholds[1].warning_threshold, None);
    }

    fn cb_with_overrides(entries: Vec<ModelThresholdToml>) -> ContextBudgetToml {
        ContextBudgetToml {
            enabled: true,
            model_thresholds: entries,
            ..ContextBudgetToml::default()
        }
    }

    #[test]
    fn threshold_override_matches_model_id_substring() {
        let cb = cb_with_overrides(vec![ModelThresholdToml {
            model: "kimi".to_string(),
            warning_threshold: Some(0.60),
            critical_threshold: Some(0.78),
        }]);
        // Case-insensitive substring of the full model id matches.
        let hit = cb
            .threshold_override_for(Some("Kimi-K2-0905-preview"), "moonshot")
            .expect("kimi substring matches model id");
        assert_eq!(hit.warning_threshold, Some(0.60));
    }

    #[test]
    fn threshold_override_matches_provider_key() {
        let cb = cb_with_overrides(vec![ModelThresholdToml {
            model: "kimi".to_string(),
            warning_threshold: Some(0.60),
            ..ModelThresholdToml::default()
        }]);
        // No model id, but the provider key carries the family name.
        let hit = cb
            .threshold_override_for(None, "kimi-cn")
            .expect("provider key matches");
        assert_eq!(hit.warning_threshold, Some(0.60));
    }

    #[test]
    fn threshold_override_first_match_wins() {
        let cb = cb_with_overrides(vec![
            ModelThresholdToml {
                model: "claude".to_string(),
                warning_threshold: Some(0.72),
                ..ModelThresholdToml::default()
            },
            ModelThresholdToml {
                model: "sonnet".to_string(),
                warning_threshold: Some(0.65),
                ..ModelThresholdToml::default()
            },
        ]);
        // Both entries match "claude-sonnet-4-6"; declaration order wins.
        let hit = cb
            .threshold_override_for(Some("claude-sonnet-4-6"), "anthropic")
            .expect("first matching entry wins");
        assert_eq!(hit.warning_threshold, Some(0.72));
    }

    #[test]
    fn threshold_override_none_when_no_match_or_empty() {
        let cb = cb_with_overrides(vec![ModelThresholdToml {
            model: "kimi".to_string(),
            warning_threshold: Some(0.60),
            ..ModelThresholdToml::default()
        }]);
        assert!(
            cb.threshold_override_for(Some("claude-sonnet-4-6"), "anthropic")
                .is_none(),
            "non-matching model → None (global thresholds apply)"
        );
        // Empty matcher string never matches (guards a blank `model = ""`).
        let blank = cb_with_overrides(vec![ModelThresholdToml {
            model: "  ".to_string(),
            warning_threshold: Some(0.5),
            ..ModelThresholdToml::default()
        }]);
        assert!(blank
            .threshold_override_for(Some("anything"), "p")
            .is_none());
        // No overrides at all → None.
        let none = cb_with_overrides(vec![]);
        assert!(none
            .threshold_override_for(Some("kimi-k2"), "moonshot")
            .is_none());
    }

    #[test]
    fn strategy_section_defaults_to_enabled() {
        // `[strategy]` present but `enabled` omitted → enabled = true, so the
        // feature is on by default (opt-out off-switch, not opt-in). This is the
        // one deviation from `[context_budget]`, which defaults to disabled.
        let p: Probe = toml::from_str("[strategy]\n").expect("toml parses");
        let s = p.strategy.expect("section present");
        assert!(s.enabled);
        // Planner model is opt-in — unset by default → planner reuses executor.
        assert!(s.planner_model.is_none());
    }

    #[test]
    fn strategy_default_is_enabled() {
        // `StrategyToml::default()` (used by `..StrategyToml::default()` in test
        // literals and by the absent-field path) must also yield enabled = true.
        let s = StrategyToml::default();
        assert!(s.enabled);
        assert!(s.planner_model.is_none());
    }

    #[test]
    fn strategy_planner_model_parses() {
        let toml_str = "[strategy]\nenabled = true\nplanner_model = \"claude-opus-4-8\"\n";
        let p: Probe = toml::from_str(toml_str).expect("toml parses");
        let s = p.strategy.expect("section present");
        assert!(s.enabled);
        assert_eq!(s.planner_model.as_deref(), Some("claude-opus-4-8"));
    }

    #[test]
    fn strategy_enabled_false_parses() {
        // The off-switch: explicitly disable the planner without removing the
        // section (A/B + Future-Proof escape valve, spec §2/§9).
        let p: Probe = toml::from_str("[strategy]\nenabled = false\n").expect("toml parses");
        let s = p.strategy.expect("section present");
        assert!(!s.enabled);
    }

    #[test]
    fn strategy_plan_naked_loop_defaults_true() {
        // Section present but field omitted ⇒ default true.
        let s: StrategyToml = toml::from_str("enabled = true").unwrap();
        assert!(s.plan_naked_loop);
        // Default impl ⇒ true.
        assert!(StrategyToml::default().plan_naked_loop);
    }

    #[test]
    fn strategy_plan_naked_loop_parses_false() {
        let s: StrategyToml = toml::from_str("plan_naked_loop = false").unwrap();
        assert!(!s.plan_naked_loop);
    }
}

#[cfg(test)]
mod plan_team_tests {
    use super::StrategyToml;

    #[test]
    fn plan_team_defaults_on() {
        assert!(
            StrategyToml::default().plan_team,
            "Default plan_team must be true"
        );
        let from_empty: StrategyToml = serde_json::from_str("{}").unwrap();
        assert!(
            from_empty.plan_team,
            "missing plan_team deserializes to true"
        );
    }
}
