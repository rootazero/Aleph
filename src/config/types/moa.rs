//! MoA (Mixture of Agents) configuration — the `[moa]` config.toml section.
//!
//! Ported from hermes-agent's moa_config.py, adapted to typed Rust config:
//! validation happens at load/patch time instead of runtime string coercion.
//! Spec: docs/superpowers/specs/2026-07-05-moa-continuous-advisory-port-design.md

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One advisor/aggregator slot: a (provider, model) pair. `provider` must
/// name a `[providers.<key>]` entry; `"moa"` is rejected (recursion guard,
/// layer 1 of 3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MoaSlot {
    pub provider: String,
    pub model: String,
}

/// Advisor fan-out cadence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MoaFanout {
    /// Advisors re-run whenever the advisory view changes (every tool
    /// iteration). hermes default — maximally informed.
    #[default]
    PerIteration,
    /// Advisors run once per user turn (= once per run); later iterations
    /// reuse that advice. The original MoA shape.
    UserTurn,
}

/// One named MoA preset.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MoaPreset {
    /// `false` = skip advisors entirely; the aggregator acts alone.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Advisor slots, fanned out in parallel on each consultation.
    #[serde(default)]
    pub advisors: Vec<MoaSlot>,
    /// The acting model: gets the full payload + advisor guidance.
    pub aggregator: MoaSlot,
    #[serde(default)]
    pub fanout: MoaFanout,
    /// Per-advisor wall-clock budget in seconds. A timed-out advisor
    /// degrades to a labelled note (hermes has no timeout at all).
    #[serde(default = "default_advisor_timeout_secs")]
    pub advisor_timeout_secs: u64,
    /// Caps ONLY advisor output (the dominant latency lever); the acting
    /// aggregator is never capped here. `None` = provider default.
    #[serde(default)]
    pub advisor_max_tokens: Option<u32>,
    /// `None` = omit the parameter so the provider default applies.
    #[serde(default)]
    pub advisor_temperature: Option<f32>,
    #[serde(default)]
    pub aggregator_temperature: Option<f32>,
}

/// The `[moa]` section.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct MoaToml {
    /// Preset used by `/moa` one-shot and a bare `moa on`.
    #[serde(default)]
    pub default_preset: Option<String>,
    /// Gate for the heavy `MoaTurnTrace` (full advisor I/O) trace events.
    #[serde(default)]
    pub save_traces: bool,
    #[serde(default)]
    pub presets: HashMap<String, MoaPreset>,
}

const fn default_true() -> bool {
    true
}

pub(crate) const fn default_advisor_timeout_secs() -> u64 {
    120
}

impl MoaToml {
    /// Resolve a preset: explicit name > `default_preset` > the sole preset
    /// when exactly one exists. Returns the resolved key alongside the preset.
    #[must_use]
    pub fn resolve_preset(&self, name: Option<&str>) -> Option<(String, &MoaPreset)> {
        let key = name
            .map(str::to_string)
            .or_else(|| self.default_preset.clone())
            .or_else(|| {
                (self.presets.len() == 1)
                    .then(|| self.presets.keys().next().cloned())
                    .flatten()
            })?;
        self.presets.get(&key).map(|p| (key, p))
    }

    /// Validation errors; empty when valid. Layer-1 recursion guard.
    #[must_use]
    pub fn validation_errors(&self) -> Vec<String> {
        let mut errs = Vec::new();
        for (name, preset) in &self.presets {
            if name.trim().is_empty() {
                errs.push("[moa] preset name must not be empty".to_string());
            }
            let mut slots: Vec<&MoaSlot> = preset.advisors.iter().collect();
            slots.push(&preset.aggregator);
            for slot in slots {
                if slot.provider.trim().is_empty() || slot.model.trim().is_empty() {
                    errs.push(format!(
                        "[moa.presets.{name}] slot provider/model must be non-empty"
                    ));
                }
                if slot.provider.trim().eq_ignore_ascii_case("moa") {
                    errs.push(format!(
                        "[moa.presets.{name}] slots cannot reference provider 'moa' \
                         (recursive MoA is forbidden)"
                    ));
                }
            }
            if preset.enabled && preset.advisors.is_empty() {
                errs.push(format!(
                    "[moa.presets.{name}] an enabled preset needs at least one advisor"
                ));
            }
            // A 0s advisor budget makes every advisor time out instantly, so the
            // preset silently degrades to aggregator-alone — reject it as a
            // configuration mistake rather than let it fail-soft every turn.
            if preset.advisor_timeout_secs == 0 {
                errs.push(format!(
                    "[moa.presets.{name}] advisor_timeout_secs must be >= 1"
                ));
            }
            // Temperatures thread straight to the provider with no clamp
            // (fan_out.rs / provider.rs → RequestPayload::with_temperature), so an
            // out-of-range value reaches the API verbatim; the aggregator branch
            // is NOT fail-soft, so a bad value there fails the whole turn with an
            // opaque 400. Reject at the config boundary (same 0.0..=2.0 convention
            // as rig/config.rs). NOTE (audit D2): this is the WIDEST range — it
            // catches gross errors + NaN/Inf, but a provider with a narrower
            // protocol limit (e.g. anthropic 0.0..=1.0) can still 400 on a value
            // in (1.0, 2.0]. Per-protocol clamping isn't available here (a MoA
            // slot carries only a provider key, not the resolved protocol), so
            // the turn-time 400 is the self-correcting backstop for that band.
            for t in [preset.advisor_temperature, preset.aggregator_temperature]
                .into_iter()
                .flatten()
            {
                if !t.is_finite() || !(0.0..=2.0).contains(&t) {
                    errs.push(format!(
                        "[moa.presets.{name}] temperature {t} is out of range [0.0, 2.0]"
                    ));
                }
            }
            // Global distinctness: every slot (all advisors + aggregator) must be
            // a unique (provider, model) after case/whitespace normalization.
            let mut seen = std::collections::HashSet::new();
            let mut all_slots: Vec<&MoaSlot> = preset.advisors.iter().collect();
            all_slots.push(&preset.aggregator);
            for slot in all_slots {
                let key = (
                    slot.provider.trim().to_lowercase(),
                    slot.model.trim().to_lowercase(),
                );
                if !seen.insert(key) {
                    errs.push(format!(
                        "[moa.presets.{name}] duplicate slot (provider, model) — \
                         advisors and aggregator must all be distinct"
                    ));
                    break;
                }
            }
        }
        if let Some(d) = &self.default_preset {
            if !self.presets.contains_key(d) {
                errs.push(format!("[moa] default_preset '{d}' does not exist"));
            }
        }
        errs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset_toml() -> &'static str {
        r#"
default_preset = "default"

[presets.default]
advisors = [
  { provider = "openai", model = "gpt-5.5" },
  { provider = "deepseek", model = "deepseek-v4" },
]
aggregator = { provider = "anthropic", model = "claude-opus-4-8" }
"#
    }

    #[test]
    fn defaults_from_empty_toml() {
        let parsed: MoaToml = toml::from_str("").unwrap();
        assert!(parsed.presets.is_empty());
        assert!(!parsed.save_traces);
        assert_eq!(parsed.default_preset, None);
    }

    #[test]
    fn preset_missing_fields_get_defaults() {
        let parsed: MoaToml = toml::from_str(preset_toml()).unwrap();
        let p = &parsed.presets["default"];
        assert!(p.enabled);
        assert_eq!(p.fanout, MoaFanout::PerIteration);
        assert_eq!(p.advisor_timeout_secs, 120);
        assert_eq!(p.advisor_max_tokens, None);
        assert_eq!(p.advisor_temperature, None);
        assert!(parsed.validation_errors().is_empty());
    }

    #[test]
    fn fanout_parses_snake_case() {
        let parsed: MoaToml = toml::from_str(
            r#"
[presets.p]
fanout = "user_turn"
advisors = [{ provider = "a", model = "m" }]
aggregator = { provider = "b", model = "n" }
"#,
        )
        .unwrap();
        assert_eq!(parsed.presets["p"].fanout, MoaFanout::UserTurn);
    }

    #[test]
    fn recursive_moa_slot_rejected_case_insensitive() {
        for prov in ["moa", "MoA", "MOA"] {
            let cfg = MoaToml {
                presets: HashMap::from([(
                    "p".to_string(),
                    MoaPreset {
                        enabled: true,
                        advisors: vec![MoaSlot {
                            provider: prov.to_string(),
                            model: "m".to_string(),
                        }],
                        aggregator: MoaSlot {
                            provider: "anthropic".to_string(),
                            model: "n".to_string(),
                        },
                        fanout: MoaFanout::default(),
                        advisor_timeout_secs: 120,
                        advisor_max_tokens: None,
                        advisor_temperature: None,
                        aggregator_temperature: None,
                    },
                )]),
                ..MoaToml::default()
            };
            assert!(
                cfg.validation_errors()
                    .iter()
                    .any(|e| e.contains("recursive")),
                "provider {prov} must be rejected"
            );
        }
    }

    #[test]
    fn enabled_preset_without_advisors_invalid() {
        let parsed: MoaToml = toml::from_str(
            r#"
[presets.p]
aggregator = { provider = "b", model = "n" }
"#,
        )
        .unwrap();
        assert!(parsed
            .validation_errors()
            .iter()
            .any(|e| e.contains("at least one advisor")));
    }

    #[test]
    fn zero_advisor_timeout_rejected() {
        let mut cfg = preset_with(vec![slot("openai", "gpt-5.5")], slot("anthropic", "opus"));
        cfg.presets.get_mut("p").unwrap().advisor_timeout_secs = 0;
        assert!(cfg
            .validation_errors()
            .iter()
            .any(|e| e.contains("advisor_timeout_secs must be >= 1")));
    }

    #[test]
    fn unknown_default_preset_invalid() {
        let parsed: MoaToml = toml::from_str("default_preset = \"ghost\"").unwrap();
        assert!(parsed
            .validation_errors()
            .iter()
            .any(|e| e.contains("does not exist")));
    }

    #[test]
    fn resolve_preset_precedence() {
        let parsed: MoaToml = toml::from_str(preset_toml()).unwrap();
        // explicit name
        assert_eq!(parsed.resolve_preset(Some("default")).unwrap().0, "default");
        // unknown explicit name -> None
        assert!(parsed.resolve_preset(Some("ghost")).is_none());
        // default_preset fallback
        assert_eq!(parsed.resolve_preset(None).unwrap().0, "default");
        // sole-preset fallback when default_preset unset
        let mut solo = parsed.clone();
        solo.default_preset = None;
        assert_eq!(solo.resolve_preset(None).unwrap().0, "default");
    }

    fn slot(p: &str, m: &str) -> MoaSlot {
        MoaSlot {
            provider: p.into(),
            model: m.into(),
        }
    }

    fn preset_with(advisors: Vec<MoaSlot>, aggregator: MoaSlot) -> MoaToml {
        MoaToml {
            presets: HashMap::from([(
                "p".to_string(),
                MoaPreset {
                    enabled: true,
                    advisors,
                    aggregator,
                    fanout: MoaFanout::default(),
                    advisor_timeout_secs: 120,
                    advisor_max_tokens: None,
                    advisor_temperature: None,
                    aggregator_temperature: None,
                },
            )]),
            ..MoaToml::default()
        }
    }

    #[test]
    fn duplicate_advisor_slots_rejected() {
        let cfg = preset_with(
            vec![slot("openai", "gpt-5.5"), slot("openai", "gpt-5.5")],
            slot("anthropic", "claude-opus-4-8"),
        );
        assert!(cfg
            .validation_errors()
            .iter()
            .any(|e| e.contains("duplicate slot")));
    }

    #[test]
    fn aggregator_equal_to_advisor_rejected() {
        let cfg = preset_with(vec![slot("openai", "gpt-5.5")], slot("openai", "gpt-5.5"));
        assert!(cfg
            .validation_errors()
            .iter()
            .any(|e| e.contains("duplicate slot")));
    }

    #[test]
    fn dedup_is_case_and_whitespace_insensitive() {
        let cfg = preset_with(vec![slot("OpenAI", " gpt-5.5 ")], slot("openai", "gpt-5.5"));
        assert!(cfg
            .validation_errors()
            .iter()
            .any(|e| e.contains("duplicate slot")));
    }

    #[test]
    fn all_distinct_slots_pass_dedup() {
        let cfg = preset_with(
            vec![slot("openai", "gpt-5.5"), slot("deepseek", "deepseek-v4")],
            slot("anthropic", "claude-opus-4-8"),
        );
        assert!(!cfg
            .validation_errors()
            .iter()
            .any(|e| e.contains("duplicate slot")));
    }
}
