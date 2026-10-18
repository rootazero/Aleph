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

pub use aleph_slot_api::ai::{normalize_think_level, ThinkLevel};

#[doc(hidden)]
pub use aleph_slot_api::ai::{dispatch_think_level, select_think_level_copy};

/// Session-metadata key under which a chosen thinking level is persisted, so
/// the choice outlives the turn that carried it (later turns send nothing; a
/// page reload reads the session back).
///
/// Twin of [`crate::config::types::policies::EXEC_TIER_SESSION_KEY`] — same
/// mechanism, same lifetime, same "the user picked this, honour it" contract.
pub const THINK_LEVEL_SESSION_KEY: &str = "think_level";

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
    fn builtin_ladder_matches_api_vocabulary() {
        let wire: Vec<&str> = builtin_think_levels().iter().map(|p| p.id).collect();
        let enumerated: Vec<&str> = ThinkLevel::ALL.iter().map(|l| l.id()).collect();
        assert_eq!(
            wire, enumerated,
            "the ladder shipped to surfaces must be ALL, in ALL's order"
        );
    }
}
