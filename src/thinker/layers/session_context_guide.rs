//! `SessionContextGuideLayer` — inject compressed-summary usage guide (priority 1750)
//!
//! Sits just after `MemoryProtocolLayer` (1745).
//! Only injects content when the conversation history contains at least one
//! `<session_context>` block (i.e. `input.has_session_summaries` is true).
//! For short sessions with no compaction this layer is a no-op with zero cost.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

/// Injects a brief guide into the system prompt explaining how to work with
/// compressed session summaries that appear as `<session_context>` blocks.
pub struct SessionContextGuideLayer;

impl PromptLayer for SessionContextGuideLayer {
    fn name(&self) -> &'static str {
        "session_context_guide"
    }

    fn priority(&self) -> u32 {
        1750
    }

    fn stability(&self) -> LayerStability {
        LayerStability::Dynamic
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        !matches!(mode, PromptMode::Minimal)
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        if !input.has_session_summaries {
            return;
        }
        output.push_str(
            "\n\n## Session Context Notes\n\
            Messages tagged with <session_context> are compressed summaries of earlier conversation.\n\
            - Summaries preserve key decisions and results but omit low-level details\n\
            - If you need specific details (code, error messages, configs), use memory_search with scope=\"current_session\"\n\
            - Do not guess specific details from summaries — search first when uncertain\n",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn metadata() {
        let layer = SessionContextGuideLayer;
        assert_eq!(layer.name(), "session_context_guide");
        assert_eq!(layer.priority(), 1750);
        assert!(layer.paths().contains(&AssemblyPath::Basic));
        assert_eq!(layer.stability(), LayerStability::Dynamic);
    }

    #[test]
    fn skips_when_no_session_summaries() {
        let layer = SessionContextGuideLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(
            out.is_empty(),
            "should not inject when has_session_summaries is false"
        );
    }

    #[test]
    fn injects_guide_when_summaries_present() {
        let layer = SessionContextGuideLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_session_summaries(true);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(
            out.contains("Session Context Notes"),
            "should inject guide header"
        );
        assert!(
            out.contains("session_context"),
            "should mention the xml tag"
        );
        assert!(
            out.contains("memory_search"),
            "should mention memory_search tool"
        );
    }

    #[test]
    fn supports_full_and_compact_not_minimal() {
        let layer = SessionContextGuideLayer;
        assert!(layer.supports_mode(PromptMode::Full));
        assert!(layer.supports_mode(PromptMode::Compact));
        assert!(!layer.supports_mode(PromptMode::Minimal));
    }
}
