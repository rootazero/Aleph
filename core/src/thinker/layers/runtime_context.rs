//! RuntimeContextLayer — micro-environmental awareness (priority 200)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct RuntimeContextLayer;

impl PromptLayer for RuntimeContextLayer {
    fn name(&self) -> &'static str { "runtime_context" }
    fn priority(&self) -> u32 { 200 }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Context]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let ctx = match input.context {
            Some(c) => c,
            None => return,
        };
        if let Some(ref runtime_ctx) = ctx.runtime_context {
            output.push_str(&runtime_ctx.to_prompt_section());
            output.push_str("\n\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_runtime_context_no_context() {
        let layer = RuntimeContextLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools); // no context
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn test_runtime_context_paths() {
        let paths = RuntimeContextLayer.paths();
        assert_eq!(paths.len(), 1);
        assert!(paths.contains(&AssemblyPath::Context));
    }
}
