//! Thinking levels — the user- and model-facing knob for LLM reasoning depth.
//!
//! # Why this matters for cost
//!
//! Reasoning tokens bill at the OUTPUT rate (the most expensive bucket), and on
//! adaptive models they are frequently the majority of a turn's output. Running
//! `low`/`off` on trivial turns and reserving `high`/`xhigh` for hard ones is
//! the single largest per-turn output-side lever available.
//!
//! # Who decides the level (R7)
//!
//! The level is **declared**, never inferred: by the user (composer / RPC
//! `thinking` param / `[execution] think_level`) or by the model itself (the
//! `self_config` tool). Nothing in Aleph reads the user's message and guesses
//! how hard the task is — that would be exactly the deterministic-code-doing-
//! the-model's-reasoning that R7 forbids.
//!
//! # This module's scope
//!
//! Just the vocabulary: the level ladder, the alias table users type, and the
//! session-metadata key the choice is persisted under. The *wire* mapping lives
//! with each protocol, which is the only layer that knows its own contract:
//!
//! - `protocols/openai_common/reasoning_effort.rs` — `reasoning_effort` + the
//!   per-model-family clamp matrix (an unsupported value is a hard 400).
//! - `protocols/anthropic/adapter.rs` — legacy `budget_tokens` vs 4.6+ adaptive
//!   `effort`.
//! - `protocols/gemini/proto_impl.rs` — `thinkingBudget` / `thinkingLevel`.
//!
//! An earlier version of this file carried a second, parallel capability matrix
//! (`supports_xhigh_thinking`, `get_supported_levels`, a `ThinkingFallbackState`
//! retry ladder, and a regex-ish parser that scraped supported levels out of
//! provider error strings). It had zero consumers, and it *contradicted* the
//! live clamp table it duplicated — it claimed o3 supports `xhigh` while
//! `reasoning_effort.rs` clamps o3 to `high`. Deleted rather than reconciled:
//! one source of truth per concern, and the wire layer owns wire facts.

use serde::{Deserialize, Serialize};

/// Session-metadata key under which a chosen thinking level is persisted, so
/// the choice outlives the turn that carried it (later turns send nothing; a
/// page reload reads the session back).
///
/// Twin of [`crate::config::types::policies::EXEC_TIER_SESSION_KEY`] — same
/// mechanism, same lifetime, same "the user picked this, honour it" contract.
pub const THINK_LEVEL_SESSION_KEY: &str = "think_level";

/// Thinking level for LLM reasoning depth control.
///
/// Six levels from no thinking to extended deep reasoning. Not every model
/// accepts every level; the per-protocol adapters clamp to the nearest value
/// the target model's family actually supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThinkLevel {
    /// No extended thinking, fastest response.
    ///
    /// NOTE for adapter authors: on several families `Off` is NOT expressible by
    /// omitting the field — Gemini 2.5 Flash and OpenAI reasoning models think
    /// by default, so omission silently buys (and bills) their default depth.
    /// `Off` must be sent as an explicit disable value on those families.
    Off,
    /// Minimal thinking, brief internal reasoning (default)
    #[default]
    Minimal,
    /// Low thinking depth
    Low,
    /// Medium thinking depth (balanced)
    Medium,
    /// High thinking depth, detailed reasoning
    High,
    /// Extended high thinking (xhigh), deepest reasoning.
    /// Only supported by specific models (e.g. GPT-5.2, Claude Opus).
    XHigh,
}

impl ThinkLevel {
    /// Canonical wire/storage id — what gets persisted in session metadata and
    /// accepted back by [`normalize_think_level`]. Mirrors `ExecTier::id()`.
    #[must_use]
    pub const fn id(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
        }
    }

    /// Every level, ordered shallow → deep.
    ///
    /// The order is the ladder itself, and it is what a picker renders top to
    /// bottom. Pinned against a new variant by `all_covers_every_variant`,
    /// which is an exhaustive `match` — adding a rung fails to compile there
    /// rather than silently shipping a surface that cannot offer it.
    pub const ALL: [Self; 6] = [
        Self::Off,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
    ];

    /// Human-facing label for UI surfaces.
    #[must_use]
    pub const fn display_name(&self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Minimal => "Minimal",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::XHigh => "Extended",
        }
    }
}

impl std::fmt::Display for ThinkLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

impl std::str::FromStr for ThinkLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        normalize_think_level(s).ok_or_else(|| format!("Unknown thinking level: '{s}'"))
    }
}

/// Normalize user-provided thinking level strings to the canonical enum.
///
/// Aliases exist because people type what they mean, not what the enum is
/// called:
/// - "think", "on", "enable" → Minimal
/// - "thinkhard" → Low
/// - "thinkharder", "harder" → Medium
/// - "ultrathink", "ultra", "max" → High
/// - "xhigh", "extended" → `XHigh`
///
/// Returns `None` for anything unrecognized — callers must REJECT rather than
/// default. Silently falling back would run the turn at a different depth (and
/// a different price) than the one the user believes they picked.
///
/// # Examples
///
/// ```rust
/// use alephcore::agents::thinking::{normalize_think_level, ThinkLevel};
///
/// assert_eq!(normalize_think_level("think"), Some(ThinkLevel::Minimal));
/// assert_eq!(normalize_think_level("ultrathink"), Some(ThinkLevel::High));
/// assert_eq!(normalize_think_level("xhigh"), Some(ThinkLevel::XHigh));
/// assert_eq!(normalize_think_level("unknown"), None);
/// ```
#[must_use]
pub fn normalize_think_level(raw: &str) -> Option<ThinkLevel> {
    let key = raw.trim().to_lowercase();

    match key.as_str() {
        // Off
        "off" | "none" | "disable" | "disabled" | "0" | "false" | "no" => Some(ThinkLevel::Off),

        // Minimal
        "minimal" | "min" | "think" | "on" | "enable" | "enabled" | "1" | "true" | "yes" => {
            Some(ThinkLevel::Minimal)
        }

        // Low
        "low" | "thinkhard" | "think-hard" | "think_hard" | "2" => Some(ThinkLevel::Low),

        // Medium
        "medium" | "med" | "mid" | "thinkharder" | "think-harder" | "think_harder" | "harder"
        | "3" => Some(ThinkLevel::Medium),

        // High
        "high" | "ultra" | "ultrathink" | "ultra-think" | "ultra_think" | "thinkhardest"
        | "think-hardest" | "highest" | "max" | "4" => Some(ThinkLevel::High),

        // XHigh
        "xhigh" | "x-high" | "x_high" | "extended" | "ext" | "5" | "extreme" => {
            Some(ThinkLevel::XHigh)
        }

        _ => None,
    }
}

/// The reasoning-depth ladder as offered to a user surface, shallow → deep.
///
/// Same contract as [`crate::config::types::policies::builtin_tiers`] and
/// `builtin_modes`: core ships ids, the surface writes the words. Rides
/// `config.get_tool_permissions` next to those two, because a composer that
/// needs three dials should not need three fetches — and because a client that
/// hard-codes this ladder is a client that keeps offering a rung this build
/// removed.
///
/// Note what is NOT here: a global default. `turn_thinking`'s precedence is
/// request > session > **no directive at all**, so a surface's "follow" row
/// means "send nothing and leave the provider on its own default", not "follow
/// a configured global". A pill that labels it "follow global" is naming a
/// setting that does not exist.
#[must_use]
pub const fn builtin_think_levels() -> &'static [crate::config::types::policies::DialPreset] {
    use crate::config::types::policies::DialPreset;
    &[
        DialPreset { id: "off" },
        DialPreset { id: "minimal" },
        DialPreset { id: "low" },
        DialPreset { id: "medium" },
        DialPreset { id: "high" },
        DialPreset { id: "xhigh" },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_minimal() {
        assert_eq!(ThinkLevel::default(), ThinkLevel::Minimal);
    }

    #[test]
    fn normalizes_canonical_names() {
        assert_eq!(normalize_think_level("off"), Some(ThinkLevel::Off));
        assert_eq!(normalize_think_level("minimal"), Some(ThinkLevel::Minimal));
        assert_eq!(normalize_think_level("low"), Some(ThinkLevel::Low));
        assert_eq!(normalize_think_level("medium"), Some(ThinkLevel::Medium));
        assert_eq!(normalize_think_level("high"), Some(ThinkLevel::High));
        assert_eq!(normalize_think_level("xhigh"), Some(ThinkLevel::XHigh));
    }

    #[test]
    fn normalizes_aliases_and_is_case_insensitive() {
        assert_eq!(
            normalize_think_level("  ULTRATHINK "),
            Some(ThinkLevel::High)
        );
        assert_eq!(normalize_think_level("Think"), Some(ThinkLevel::Minimal));
        assert_eq!(normalize_think_level("extended"), Some(ThinkLevel::XHigh));
    }

    #[test]
    fn unknown_input_is_rejected_not_defaulted() {
        assert_eq!(normalize_think_level("deeeep"), None);
        assert!("deeeep".parse::<ThinkLevel>().is_err());
    }

    /// A new rung has to reach [`ThinkLevel::ALL`] and the wire ladder, and
    /// the compiler is the only thing that can insist.
    ///
    /// The `match` is what does the work: adding a variant makes it
    /// non-exhaustive, which fails the build *here*, next to both lists. The
    /// two assertions then catch the other half — a variant added to the enum
    /// and to the match but forgotten in `ALL` or in the wire ladder, which is
    /// how a surface ends up unable to offer a level the run loop enforces.
    #[test]
    fn all_covers_every_variant() {
        for level in ThinkLevel::ALL {
            match level {
                ThinkLevel::Off
                | ThinkLevel::Minimal
                | ThinkLevel::Low
                | ThinkLevel::Medium
                | ThinkLevel::High
                | ThinkLevel::XHigh => {}
            }
        }
        assert_eq!(
            ThinkLevel::ALL.len(),
            6,
            "a rung was added to the enum without joining ALL"
        );
        let wire: Vec<&str> = builtin_think_levels().iter().map(|p| p.id).collect();
        let enumerated: Vec<&str> = ThinkLevel::ALL.iter().map(|l| l.id()).collect();
        assert_eq!(
            wire, enumerated,
            "the ladder shipped to surfaces must be ALL, in ALL's order"
        );
    }

    /// `id()` is the session-metadata storage form, so it MUST survive a
    /// write/read round-trip — otherwise a persisted choice silently decays to
    /// "unknown" on the next turn.
    #[test]
    fn id_round_trips_through_normalize() {
        for level in [
            ThinkLevel::Off,
            ThinkLevel::Minimal,
            ThinkLevel::Low,
            ThinkLevel::Medium,
            ThinkLevel::High,
            ThinkLevel::XHigh,
        ] {
            assert_eq!(
                normalize_think_level(level.id()),
                Some(level),
                "id() must parse back to itself for {level:?}"
            );
            assert_eq!(level.to_string(), level.id());
        }
    }
}
