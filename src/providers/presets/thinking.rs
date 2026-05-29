//! Declarative thinking profile per provider preset.
//!
//! Replaces the hardcoded `eq_ignore_ascii_case` ladders previously living in
//! `agents/thinking_adapter.rs` (`supports_thinking_control` and
//! `get_thinking_param_key`). Profiles are attached to a [`ProviderPreset`]
//! via [`ProviderPreset::with_thinking_profile`] so adding a new family is a
//! single declarative edit in `registry.rs`.
//!
//! Inspired by openclaw's `resolveThinkingProfile` extension hook, scaled to
//! Aleph's preset-centric architecture (no plugin tree needed).

/// Reasoning depth levels exposed to upstream callers.
///
/// Mirrors openclaw's `ProviderThinkingProfile.levels` taxonomy so that
/// vendor-specific request shapes (`thinking`, `reasoning_effort`,
/// `thinking_config`, `enable_thinking`, `enable_reasoning`, `use_thinking`)
/// all map cleanly onto a single ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThinkingLevel {
    /// Reasoning explicitly disabled.
    Off,
    /// Minimal reasoning budget (e.g. OpenAI o-series `minimal`).
    Minimal,
    /// Low reasoning budget.
    Low,
    /// Medium reasoning budget — the default for tool-using agents.
    Medium,
    /// High reasoning budget.
    High,
    /// Provider-specific "extra-high" tier (Gemini 3 Pro, Codex xhigh).
    XHigh,
    /// Adaptive / auto budget (Gemini "adaptive").
    Adaptive,
}

/// Declarative reasoning ladder + request-body parameter key for a vendor.
///
/// `levels` lists the legal levels for this family (Off is always implied at
/// runtime; explicit inclusion lets the UI render an explicit toggle).
/// `param_key` is the field name to merge into the outgoing JSON body for
/// providers that take a simple scalar (e.g. `reasoning_effort`, `thinking`).
/// Providers with richer payloads (Anthropic's thinking block, Gemini's
/// thinking_config object) still go through `ThinkingAdapter::to_provider_params`
/// — `param_key` is used by `merge_into_body` as the canonical top-level key.
#[derive(Debug, Clone, Copy)]
pub struct ThinkingProfile {
    /// Supported reasoning levels for this provider family.
    pub levels: &'static [ThinkingLevel],
    /// Top-level JSON field this provider expects in the request body.
    pub param_key: &'static str,
}

// =============================================================================
// Built-in profiles (matches the 6 families previously hardcoded in
// thinking_adapter.rs)
// =============================================================================

/// Anthropic Claude — `thinking` block, supports Off/Low/Medium/High.
pub const ANTHROPIC_THINKING: ThinkingProfile = ThinkingProfile {
    levels: &[
        ThinkingLevel::Off,
        ThinkingLevel::Low,
        ThinkingLevel::Medium,
        ThinkingLevel::High,
    ],
    param_key: "thinking",
};

/// OpenAI o-series — `reasoning_effort` scalar, supports Off..High.
pub const OPENAI_THINKING: ThinkingProfile = ThinkingProfile {
    levels: &[
        ThinkingLevel::Off,
        ThinkingLevel::Minimal,
        ThinkingLevel::Low,
        ThinkingLevel::Medium,
        ThinkingLevel::High,
    ],
    param_key: "reasoning_effort",
};

/// Google Gemini — `thinking_config` object, supports the adaptive tier.
pub const GEMINI_THINKING: ThinkingProfile = ThinkingProfile {
    levels: &[
        ThinkingLevel::Off,
        ThinkingLevel::Low,
        ThinkingLevel::Medium,
        ThinkingLevel::High,
        ThinkingLevel::Adaptive,
    ],
    param_key: "thinking_config",
};

/// DeepSeek — `enable_thinking` boolean toggle.
pub const DEEPSEEK_THINKING: ThinkingProfile = ThinkingProfile {
    levels: &[ThinkingLevel::Off, ThinkingLevel::High],
    param_key: "enable_thinking",
};

/// Volcengine Doubao / Ark — `enable_reasoning` boolean toggle.
pub const DOUBAO_THINKING: ThinkingProfile = ThinkingProfile {
    levels: &[ThinkingLevel::Off, ThinkingLevel::High],
    param_key: "enable_reasoning",
};

/// Moonshot / Kimi — `use_thinking` boolean toggle.
pub const MOONSHOT_THINKING: ThinkingProfile = ThinkingProfile {
    levels: &[ThinkingLevel::Off, ThinkingLevel::High],
    param_key: "use_thinking",
};

// =============================================================================
// Resolver — accepts both preset names ("claude", "deepseek") and protocol
// names ("anthropic", "google") that historically appeared in caller code.
// =============================================================================

/// Resolve a reasoning profile from either a preset name or a protocol alias.
///
/// Replaces the hand-rolled `eq_ignore_ascii_case` ladder that previously lived
/// in `agents/thinking_adapter.rs`. The lookup is:
///
/// 1. Try `get_preset(name)` — covers canonical names (`claude`, `openai`,
///    `gemini`, `deepseek`, `doubao`, `moonshot`) and their declared aliases
///    (`kimi`, `volcengine`, `ark`, …).
/// 2. Fall back to a tiny protocol-alias table for the two transport-layer
///    identifiers (`anthropic`, `google`) that this project deliberately
///    does NOT keep as preset aliases (see `test_technical_aliases_removed`).
///
/// Anything else returns `None`.
pub fn lookup_thinking_profile(name: &str) -> Option<ThinkingProfile> {
    if let Some(preset) = crate::providers::presets::get_preset(name) {
        return preset.thinking_profile;
    }
    // Protocol-name fallback. Intentionally restricted to the two names the
    // legacy ThinkingAdapter accepted; everything else uses preset lookup.
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "anthropic" => Some(ANTHROPIC_THINKING),
        "google" => Some(GEMINI_THINKING),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_profile_has_4_levels() {
        assert_eq!(ANTHROPIC_THINKING.levels.len(), 4);
        assert_eq!(ANTHROPIC_THINKING.param_key, "thinking");
    }

    #[test]
    fn openai_profile_includes_minimal() {
        assert!(OPENAI_THINKING.levels.contains(&ThinkingLevel::Minimal));
        assert_eq!(OPENAI_THINKING.param_key, "reasoning_effort");
    }

    #[test]
    fn gemini_profile_includes_adaptive() {
        assert!(GEMINI_THINKING.levels.contains(&ThinkingLevel::Adaptive));
    }

    #[test]
    fn binary_profiles_are_off_high_only() {
        for p in [DEEPSEEK_THINKING, DOUBAO_THINKING, MOONSHOT_THINKING] {
            assert_eq!(p.levels.len(), 2);
            assert!(p.levels.contains(&ThinkingLevel::Off));
            assert!(p.levels.contains(&ThinkingLevel::High));
        }
    }
}
