//! GenerationModelsLayer — media generation models (priority 1000)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct GenerationModelsLayer;

impl PromptLayer for GenerationModelsLayer {
    fn name(&self) -> &'static str { "generation_models" }
    fn priority(&self) -> u32 { 1000 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        if let Some(ref models) = input.config.generation_models {
            output.push_str("## Media Generation Models\n\n");
            output.push_str(models);
            output.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_generation_models_present() {
        let layer = GenerationModelsLayer;
        let config = PromptConfig {
            generation_models: Some("- DALL-E 3\n- Stable Diffusion".to_string()),
            ..Default::default()
        };
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Media Generation Models"));
        assert!(out.contains("DALL-E 3"));
    }

    #[test]
    fn test_generation_models_absent() {
        let layer = GenerationModelsLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }
}
