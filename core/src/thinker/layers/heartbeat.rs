//! Heartbeat layer — progress reporting guidance for long-running tasks (priority 710).

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct HeartbeatLayer;

impl PromptLayer for HeartbeatLayer {
    fn name(&self) -> &'static str { "heartbeat" }
    fn priority(&self) -> u32 { 710 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Soul, AssemblyPath::Context, AssemblyPath::Cached]
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn inject(&self, output: &mut String, _input: &LayerInput) {
        output.push_str("## Progress Reporting\n\n");
        output.push_str("For long-running tasks (multi-step plans, large file operations):\n");
        output.push_str("- Report progress after completing each major step\n");
        output.push_str("- Use structured progress format: [step N/total] description\n");
        output.push_str("- If a step takes unusually long, report intermediate status\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn layer_metadata() {
        let layer = HeartbeatLayer;
        assert_eq!(layer.name(), "heartbeat");
        assert_eq!(layer.priority(), 710);

        let paths = layer.paths();
        assert!(paths.contains(&AssemblyPath::Basic));
        assert!(paths.contains(&AssemblyPath::Soul));
        assert!(paths.contains(&AssemblyPath::Context));
        assert!(paths.contains(&AssemblyPath::Cached));
        assert!(!paths.contains(&AssemblyPath::Hydration));
    }

    #[test]
    fn injects_progress_guidance() {
        let layer = HeartbeatLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("Progress Reporting"));
        assert!(out.contains("[step N/total]"));
    }
}
