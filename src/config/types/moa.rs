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
///
/// Wire form is a STRING (`"per_iteration"` / `"user_turn"` / `"every_n:3"`),
/// not a serde-derived enum: `EveryN` carries a payload, and a derived
/// representation would spell it `{ "every_n": 3 }` — an object where every
/// other cadence is a bare string. One scalar keeps TOML, the `moa` tool
/// schema, the `moa.*` RPCs and the Panel `<select>` all agreeing on one
/// shape, and keeps the two pre-existing spellings byte-identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MoaFanout {
    /// Advisors re-run whenever the advisory view changes (every tool
    /// iteration). Maximally informed, and the most expensive: advisor
    /// latency and spend both multiply by the tool-loop depth.
    #[default]
    PerIteration,
    /// Advisors run once per user turn (= once per run); later iterations
    /// reuse that advice. The original MoA shape, and the cheapest.
    UserTurn,
    /// The middle ground: advisors run on the first state advance of the run
    /// and then every `N`-th one after it; the iterations in between reuse
    /// the last advice. `N >= 2` by construction — [`FromStr`] folds
    /// `every_n:1` into [`PerIteration`] (they are the same cadence) and
    /// rejects `every_n:0`.
    ///
    /// [`FromStr`]: std::str::FromStr
    EveryN(u32),
}

impl std::fmt::Display for MoaFanout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PerIteration => f.write_str("per_iteration"),
            Self::UserTurn => f.write_str("user_turn"),
            Self::EveryN(n) => write!(f, "every_n:{n}"),
        }
    }
}

/// Accepted spellings, quoted once for every error message and the schema.
const FANOUT_FORMS: &str = "`per_iteration`, `user_turn`, or `every_n:<N>` (N >= 2)";

impl std::str::FromStr for MoaFanout {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let raw = s.trim();
        match raw {
            "per_iteration" => Ok(Self::PerIteration),
            "user_turn" => Ok(Self::UserTurn),
            _ => {
                let digits = raw.strip_prefix("every_n:").ok_or_else(|| {
                    format!("unknown MoA fanout '{raw}' — expected {FANOUT_FORMS}")
                })?;
                let every: u32 = digits.trim().parse().map_err(|_| {
                    format!(
                        "MoA fanout 'every_n:{}' needs an integer N >= 2",
                        digits.trim()
                    )
                })?;
                match every {
                    // Never consulting advisors is what `enabled = false` is
                    // for; silently accepting it here would look like a
                    // cadence and behave like an off switch.
                    0 => Err("MoA fanout `every_n:0` would never consult advisors — \
                         use `enabled = false` to run the aggregator alone, or \
                         `user_turn` to consult once per run"
                        .to_string()),
                    // Degenerate: "every 1st iteration" IS per-iteration.
                    // Collapsing keeps one canonical spelling per cadence, so
                    // the cache/cadence code never has to handle N == 1.
                    1 => Ok(Self::PerIteration),
                    n => Ok(Self::EveryN(n)),
                }
            }
        }
    }
}

impl Serialize for MoaFanout {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for MoaFanout {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for MoaFanout {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "MoaFanout".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "title": "MoaFanout",
            "description":
                "Advisor fan-out cadence: `per_iteration` (re-consult on every \
                 tool iteration), `user_turn` (consult once per run), or \
                 `every_n:<N>` with N >= 2 (consult on the first iteration, \
                 then every Nth).",
            "examples": ["per_iteration", "user_turn", "every_n:3"]
        })
    }
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
            preset_validation_errors(name, preset, &mut errs);
        }
        if let Some(d) = &self.default_preset {
            if !self.presets.contains_key(d) {
                errs.push(format!("[moa] default_preset '{d}' does not exist"));
            }
        }
        errs
    }
}

/// Per-preset validation errors, appended to `errs`. Extracted from
/// [`MoaConfig::validation_errors`] so each rule reads top-to-bottom.
fn preset_validation_errors(name: &str, preset: &MoaPreset, errs: &mut Vec<String>) {
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
    // `FromStr` already folds `every_n:1` into `PerIteration` and
    // rejects `every_n:0`, so a parsed config can never land here. A
    // preset built as a Rust literal (the `moa` tool, `MoaPresetStore`,
    // tests) bypasses that parse, and `EveryN(0)` would divide by zero
    // in the cadence check — keep the boundary honest on both paths.
    if let MoaFanout::EveryN(n) = preset.fanout {
        if n < 2 {
            errs.push(format!(
                "[moa.presets.{name}] fanout every_n:{n} must have N >= 2 \
                 (use per_iteration for every iteration)"
            ));
        }
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

    fn fanout_toml(spelling: &str) -> String {
        format!(
            r#"
[presets.p]
fanout = "{spelling}"
advisors = [{{ provider = "a", model = "m" }}]
aggregator = {{ provider = "b", model = "n" }}
"#
        )
    }

    #[test]
    fn every_n_parses_and_round_trips_through_the_wire_string() {
        let parsed: MoaToml = toml::from_str(&fanout_toml("every_n:3")).unwrap();
        assert_eq!(parsed.presets["p"].fanout, MoaFanout::EveryN(3));
        assert!(parsed.validation_errors().is_empty());
        // The wire form is one scalar in both directions — the Panel `<select>`
        // and the `moa` tool both round-trip a preset through it.
        assert_eq!(
            serde_json::to_value(MoaFanout::EveryN(3)).unwrap(),
            serde_json::json!("every_n:3")
        );
        assert_eq!(
            serde_json::to_value(MoaFanout::PerIteration).unwrap(),
            serde_json::json!("per_iteration")
        );
        assert_eq!(
            serde_json::from_value::<MoaFanout>(serde_json::json!("user_turn")).unwrap(),
            MoaFanout::UserTurn
        );
    }

    #[test]
    fn every_n_one_collapses_and_zero_is_rejected() {
        // "every 1st iteration" IS per-iteration — one canonical spelling per
        // cadence means the cadence arithmetic never sees N == 1.
        let one: MoaToml = toml::from_str(&fanout_toml("every_n:1")).unwrap();
        assert_eq!(one.presets["p"].fanout, MoaFanout::PerIteration);

        for bad in ["every_n:0", "every_n:", "every_n:abc", "bogus", "everyn:2"] {
            let err = toml::from_str::<MoaToml>(&fanout_toml(bad))
                .expect_err(&format!("`{bad}` must not parse"))
                .to_string();
            assert!(
                err.contains("every_n") || err.contains("unknown MoA fanout"),
                "`{bad}` must fail with an actionable message, got: {err}"
            );
        }
    }

    #[test]
    fn every_n_below_two_is_rejected_when_built_as_a_literal() {
        // `FromStr` cannot be bypassed by config, but the `moa` tool and
        // `MoaPresetStore` build `MoaPreset` values directly.
        let mut cfg = preset_with(vec![slot("openai", "gpt-5.5")], slot("anthropic", "opus"));
        cfg.presets.get_mut("p").unwrap().fanout = MoaFanout::EveryN(0);
        assert!(cfg
            .validation_errors()
            .iter()
            .any(|e| e.contains("must have N >= 2")));
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
