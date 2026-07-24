use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Serde default for the open-loop sub-flags: on. The whole pipeline stays
/// gated behind `enabled` (default off — a per-session-end LLM call must
/// remain opt-in), but flipping that single flag lights the full lessons +
/// open-loops pipeline without hunting for two more sub-flags.
fn default_open_loop_flag() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReflectionConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "super::defaults::default_reflection_min_turns")]
    pub min_turns: u32,
    #[serde(default = "super::defaults::default_reflection_min_chars")]
    pub min_user_chars: u32,
    #[serde(default = "super::defaults::default_reflection_cooldown")]
    pub cooldown_minutes: u32,
    /// Extract this session's *open loops* — unresolved questions, promised
    /// follow-ups, or incomplete tasks — during the same session-end reflection
    /// LLM call (no extra call), and persist them to
    /// `~/.aleph/agents/<id>/OPEN_LOOPS.md`. Default on; requires `enabled`
    /// (which stays default-off), so the default config still makes zero
    /// LLM calls.
    #[serde(default = "default_open_loop_flag")]
    pub open_loop_tracking: bool,
    /// Inject the persisted open loops into the next session's curated context
    /// so the agent proactively picks them back up (R5 — "AI comes to you").
    /// Default on; only meaningful alongside `open_loop_tracking`.
    #[serde(default = "default_open_loop_flag")]
    pub open_loop_inject_prompt: bool,
}

impl Default for ReflectionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_turns: super::defaults::default_reflection_min_turns(),
            min_user_chars: super::defaults::default_reflection_min_chars(),
            cooldown_minutes: super::defaults::default_reflection_cooldown(),
            open_loop_tracking: default_open_loop_flag(),
            open_loop_inject_prompt: default_open_loop_flag(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One-flag pipeline: `enabled` stays opt-out (LLM spend), while both
    /// open-loop sub-flags default on so flipping `enabled` alone lights the
    /// whole lessons + open-loops pipeline.
    #[test]
    fn defaults_keep_enabled_off_but_open_loop_flags_on() {
        let cfg = ReflectionConfig::default();
        assert!(!cfg.enabled, "reflection LLM call must stay opt-in");
        assert!(cfg.open_loop_tracking);
        assert!(cfg.open_loop_inject_prompt);
    }

    /// Serde defaults mirror `Default`: a bare `{}` (old configs without the
    /// open-loop fields) now reads the sub-flags as true, while explicit
    /// `false` values are preserved.
    #[test]
    fn serde_defaults_match_and_explicit_false_is_kept() {
        let bare: ReflectionConfig = serde_json::from_str("{}").unwrap();
        assert!(!bare.enabled);
        assert!(bare.open_loop_tracking);
        assert!(bare.open_loop_inject_prompt);

        let explicit: ReflectionConfig = serde_json::from_str(
            r#"{"open_loop_tracking": false, "open_loop_inject_prompt": false}"#,
        )
        .unwrap();
        assert!(!explicit.open_loop_tracking);
        assert!(!explicit.open_loop_inject_prompt);
    }
}
