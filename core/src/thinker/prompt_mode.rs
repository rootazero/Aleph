//! PromptMode — controls which layers participate in prompt assembly.
//!
//! Used by [`PromptLayer::supports_mode`] to filter layers at assembly time,
//! enabling lightweight prompts for token-constrained scenarios.

/// Controls the verbosity tier of the assembled system prompt.
///
/// The pipeline can be asked to produce a Full, Compact, or Minimal
/// prompt.  Each layer declares which modes it supports via
/// [`PromptLayer::supports_mode`]; the pipeline skips layers that
/// return `false` for the active mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
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
}
