//! CitationStandardsLayer — memory citation standards (priority 900)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct CitationStandardsLayer;

impl PromptLayer for CitationStandardsLayer {
    fn name(&self) -> &'static str { "citation_standards" }
    fn priority(&self) -> u32 { 900 }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Soul, AssemblyPath::Context]
    }
    fn inject(&self, output: &mut String, _input: &LayerInput) {
        output.push_str("## Citation Standards\n\n");
        output.push_str("When referencing information from memory or knowledge base:\n");
        output.push_str("- Include source reference in format: `[Source: <path>#<id>]` or `[Source: <path>#L<line>]`\n");
        output.push_str("- Sources are provided in the context metadata — do not fabricate source paths\n");
        output.push_str("- If multiple sources support a claim, cite the most specific one\n");
        output.push_str("- For real-time observations (current tool output, live data), no citation needed\n");
        output.push_str("- For recalled facts, prior decisions, or historical context, citation is mandatory\n\n");
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
        assert!(out.contains("citation is mandatory"));
    }

    #[test]
    fn test_citation_standards_paths() {
        let paths = CitationStandardsLayer.paths();
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&AssemblyPath::Soul));
        assert!(paths.contains(&AssemblyPath::Context));
        assert!(!paths.contains(&AssemblyPath::Basic));
    }
}
