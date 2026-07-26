//! `CitationStandardsLayer` — memory citation standards (priority 900)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct CitationStandardsLayer;

impl PromptLayer for CitationStandardsLayer {
    fn name(&self) -> &'static str {
        "citation_standards"
    }
    fn priority(&self) -> u32 {
        900
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        // `Cached` is the live main-agent-loop path
        // (`build_system_prompt_cached_with_mode`). Without it, the citation
        // standards — which govern how the model attributes recalled memory
        // (`[Source: <path>#<id>]`) — never reached production prompts. Stable +
        // Full-only, so it rides the cacheable prefix at zero per-request cost.
        &[AssemblyPath::Cached]
    }
    fn inject(&self, output: &mut String, _input: &LayerInput) {
        // The `[Source: …]` token is the non-guessable protocol — keep it. The
        // surrounding mandatory-vs-optional bullet lecture was compressed
        // (§1.1 prune-the-prompt).
        output.push_str("## Citation Standards\n\n");
        output.push_str(
            "When you state a recalled fact, prior decision, or anything from memory / the \
             knowledge base, attribute it with `[Source: <path>#<id>]` (or `#L<line>`) using the \
             paths given in context metadata — never fabricate a source. Live tool output and \
             direct observations need no citation.\n\n",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_citation_standards_content() {
        let layer = CitationStandardsLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Citation Standards"));
        assert!(out.contains("[Source: <path>#<id>]"));
        assert!(out.contains("recalled fact"));
        assert!(out.contains("never fabricate a source"));
    }

    #[test]
    fn test_citation_standards_paths() {
        let paths = CitationStandardsLayer.paths();
        assert_eq!(paths.len(), 1);
        // Must participate in the live main-loop path so citation rules
        // actually reach production prompts.
        assert!(paths.contains(&AssemblyPath::Cached));
        assert!(!paths.contains(&AssemblyPath::Basic));
    }
}
