//! `PromptMode` — controls which layers participate in prompt assembly.
//!
//! Used by [`PromptLayer::supports_mode`] to filter layers at assembly time,
//! enabling lightweight prompts for token-constrained scenarios.
//!
//! # Do not auto-select this to save money
//!
//! The recurring proposal is to pick the mode from the model's declared context
//! window or price tier. It was measured (`aleph-server prompt-size --path
//! cached`, 2026-07-14) and it does not pay:
//!
//! | mode      | static scaffold | what survives                        |
//! |-----------|-----------------|--------------------------------------|
//! | `Full`    | ~2,696 tok      | everything                           |
//! | `Compact` | ~1,019 tok      | `tools` + `memory_protocol`          |
//! | `Minimal` | ~22 tok (74 B!) | `tools`                              |
//!
//! The whole system prompt — stable zone AND dynamic zone — sits inside the
//! provider's cached prefix: the Anthropic adapter puts `cache_control` on the
//! most recent *messages*, and `system` precedes `messages` in the request, so
//! from the second call of a session onward every one of those bytes is a cache
//! READ at 0.1x. `Compact`'s 1,677-token saving is therefore worth ~168
//! token-equivalents per call — about $0.0005 at Sonnet input rates, against
//! runs that cost dollars. Roughly 0.02%.
//!
//! What it costs is not 0.02%. `Compact` drops `role`, `environment`,
//! `guidelines`, `special_actions`, `citation_standards` and `heartbeat` —
//! the agent's identity and its operating rules. `Minimal` leaves 74 bytes.
//! These are blunt instruments for an operator who has decided to accept that;
//! they are not a cost knob, and nothing should select them on a user's behalf.
//!
//! The genuine case — *the prompt does not fit a small window* — is already
//! handled, and handled better, by `TokenBudget::from_context_window`
//! (`prompt_budget.rs`, wired at `harness_bridge/prompt_build.rs`): it sizes a
//! character budget to the model's actual window and trims the dynamic suffix,
//! degrading gracefully instead of deleting whole layers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Controls the verbosity tier of the assembled system prompt.
///
/// The pipeline can be asked to produce a Full, Compact, or Minimal
/// prompt.  Each layer declares which modes it supports via
/// [`PromptLayer::supports_mode`]; the pipeline skips layers that
/// return `false` for the active mode.
///
/// Serializes to lowercase (`"full"` / `"compact"` / `"minimal"`) so it can be
/// read directly from `[execution] prompt_mode` in `aleph.toml`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PromptMode {
    /// All layers participate — maximum context.
    #[default]
    Full,
    /// Heavy/verbose layers are excluded to save tokens.
    Compact,
    /// Only essential layers (tools, response format, language).
    Minimal,
}

impl PromptMode {
    /// Human-readable label for logging / debug.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Compact => "compact",
            Self::Minimal => "minimal",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_full() {
        assert_eq!(PromptMode::default(), PromptMode::Full);
    }

    #[test]
    fn labels() {
        assert_eq!(PromptMode::Full.label(), "full");
        assert_eq!(PromptMode::Compact.label(), "compact");
        assert_eq!(PromptMode::Minimal.label(), "minimal");
    }

    #[test]
    fn serde_uses_lowercase_labels() {
        // Wire format mirrors `label()` so config and telemetry agree.
        for mode in [PromptMode::Full, PromptMode::Compact, PromptMode::Minimal] {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(json, format!("\"{}\"", mode.label()));
            let back: PromptMode = serde_json::from_str(&json).unwrap();
            assert_eq!(back, mode);
        }
    }
}
