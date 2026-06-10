//! PromptMode — controls which layers participate in prompt assembly.
//!
//! Used by [`PromptLayer::supports_mode`] to filter layers at assembly time,
//! enabling lightweight prompts for token-constrained scenarios.

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
    pub fn label(&self) -> &'static str {
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
