//! PromptPipeline — composable prompt assembly engine
//!
//! The pipeline holds an ordered list of [`PromptLayer`] implementations
//! and executes them in priority order for a given [`AssemblyPath`].

use super::layers::*;
use super::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use super::prompt_mode::PromptMode;

/// Composable prompt assembly engine.
///
/// Layers are sorted by priority at construction time.  Calling
/// [`execute`](Self::execute) runs every layer whose declared paths
/// include the requested path, appending each layer's output to a
/// single `String`.
pub struct PromptPipeline {
    layers: Vec<Box<dyn PromptLayer>>,
}

impl PromptPipeline {
    /// Create a new pipeline, sorting layers by ascending priority.
    pub fn new(mut layers: Vec<Box<dyn PromptLayer>>) -> Self {
        layers.sort_by_key(|l| l.priority());
        Self { layers }
    }

    /// Execute the pipeline for the given `path` and `input`.
    ///
    /// Returns the assembled system prompt string.
    pub fn execute(&self, path: AssemblyPath, input: &LayerInput) -> String {
        let mut output = String::with_capacity(16384);
        for layer in &self.layers {
            if layer.paths().contains(&path) {
                layer.inject(&mut output, input);
            }
        }
        output
    }

    /// Execute pipeline with mode filtering (path + mode).
    pub fn execute_with_mode(
        &self,
        path: AssemblyPath,
        input: &LayerInput,
        mode: PromptMode,
    ) -> String {
        let mut output = String::with_capacity(16384);
        for layer in &self.layers {
            if layer.paths().contains(&path) && layer.supports_mode(mode) {
                layer.inject(&mut output, input);
            }
        }
        output
    }

    /// Create a pipeline pre-loaded with the 22 default layers.
    ///
    /// Layer order (by priority):
    ///   50  SoulLayer
    ///   75  ProfileLayer
    ///  100  RoleLayer
    ///  200  RuntimeContextLayer
    ///  300  EnvironmentLayer
    ///  400  RuntimeCapabilitiesLayer
    ///  500  ToolsLayer + HydratedToolsLayer
    ///  505  PoePromptLayer
    ///  600  SecurityLayer
    ///  700  ProtocolTokensLayer
    ///  800  OperationalGuidelinesLayer
    ///  900  CitationStandardsLayer
    /// 1000  GenerationModelsLayer
    /// 1050  SkillInstructionsLayer
    /// 1100  SpecialActionsLayer
    /// 1200  ResponseFormatLayer
    /// 1300  GuidelinesLayer
    /// 1350  ThinkingGuidanceLayer
    /// 1400  SkillModeLayer
    /// 1500  CustomInstructionsLayer
    /// 1600  LanguageLayer
    pub fn default_layers() -> Self {
        Self::new(vec![
            Box::new(SoulLayer),
            Box::new(ProfileLayer),
            Box::new(RoleLayer),
            Box::new(RuntimeContextLayer),
            Box::new(EnvironmentLayer),
            Box::new(RuntimeCapabilitiesLayer),
            Box::new(ToolsLayer),
            Box::new(HydratedToolsLayer),
            Box::new(crate::poe::PoePromptLayer),
            Box::new(SecurityLayer),
            Box::new(ProtocolTokensLayer),
            Box::new(OperationalGuidelinesLayer),
            Box::new(CitationStandardsLayer),
            Box::new(GenerationModelsLayer),
            Box::new(SkillInstructionsLayer),
            Box::new(SpecialActionsLayer),
            Box::new(ResponseFormatLayer),
            Box::new(GuidelinesLayer),
            Box::new(ThinkingGuidanceLayer),
            Box::new(SkillModeLayer),
            Box::new(CustomInstructionsLayer),
            Box::new(LanguageLayer),
        ])
    }

    /// Number of registered layers (test helper).
    #[cfg(test)]
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    // --- helpers -----------------------------------------------------------

    struct StubLayer {
        name: &'static str,
        priority: u32,
        paths: &'static [AssemblyPath],
        text: &'static str,
    }

    impl PromptLayer for StubLayer {
        fn name(&self) -> &'static str { self.name }
        fn priority(&self) -> u32 { self.priority }
        fn paths(&self) -> &'static [AssemblyPath] { self.paths }
        fn inject(&self, output: &mut String, _input: &LayerInput) {
            output.push_str(self.text);
        }
    }

    fn stub(name: &'static str, prio: u32, paths: &'static [AssemblyPath], text: &'static str) -> Box<dyn PromptLayer> {
        Box::new(StubLayer { name, priority: prio, paths, text })
    }

    // --- tests -------------------------------------------------------------

    #[test]
    fn layers_sorted_by_priority() {
        let pipeline = PromptPipeline::new(vec![
            stub("c", 30, &[AssemblyPath::Basic], "C"),
            stub("a", 10, &[AssemblyPath::Basic], "A"),
            stub("b", 20, &[AssemblyPath::Basic], "B"),
        ]);

        let config = PromptConfig::default();
        let tools: Vec<crate::agent_loop::ToolInfo> = vec![];
        let input = LayerInput::basic(&config, &tools);
        let result = pipeline.execute(AssemblyPath::Basic, &input);

        assert_eq!(result, "ABC");
    }

    #[test]
    fn path_filtering() {
        let pipeline = PromptPipeline::new(vec![
            stub("basic_only", 10, &[AssemblyPath::Basic], "BASIC"),
            stub("soul_only",  20, &[AssemblyPath::Soul],  "SOUL"),
            stub("both",       30, &[AssemblyPath::Basic, AssemblyPath::Soul], "BOTH"),
        ]);

        let config = PromptConfig::default();
        let tools: Vec<crate::agent_loop::ToolInfo> = vec![];
        let input = LayerInput::basic(&config, &tools);

        let basic_result = pipeline.execute(AssemblyPath::Basic, &input);
        assert_eq!(basic_result, "BASICBOTH");

        let soul_result = pipeline.execute(AssemblyPath::Soul, &input);
        assert_eq!(soul_result, "SOULBOTH");
    }

    #[test]
    fn empty_pipeline_returns_empty_string() {
        let pipeline = PromptPipeline::new(vec![]);
        let config = PromptConfig::default();
        let tools: Vec<crate::agent_loop::ToolInfo> = vec![];
        let input = LayerInput::basic(&config, &tools);

        assert_eq!(pipeline.execute(AssemblyPath::Basic, &input), "");
        assert_eq!(pipeline.layer_count(), 0);
    }

    #[test]
    fn layer_count_matches() {
        let pipeline = PromptPipeline::new(vec![
            stub("a", 1, &[AssemblyPath::Basic], ""),
            stub("b", 2, &[AssemblyPath::Basic], ""),
        ]);
        assert_eq!(pipeline.layer_count(), 2);
    }

    #[test]
    fn test_default_layers_count() {
        let pipeline = PromptPipeline::default_layers();
        assert_eq!(pipeline.layer_count(), 22);
    }

    #[test]
    fn test_default_layers_sorted() {
        let pipeline = PromptPipeline::default_layers();
        let priorities: Vec<u32> = pipeline.layers.iter().map(|l| l.priority()).collect();
        assert!(priorities.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn no_matching_path_returns_empty() {
        let pipeline = PromptPipeline::new(vec![
            stub("soul_only", 10, &[AssemblyPath::Soul], "SOUL"),
        ]);

        let config = PromptConfig::default();
        let tools: Vec<crate::agent_loop::ToolInfo> = vec![];
        let input = LayerInput::basic(&config, &tools);

        assert_eq!(pipeline.execute(AssemblyPath::Basic, &input), "");
    }
}

#[cfg(test)]
mod mode_tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_mode::PromptMode;

    #[test]
    fn full_mode_includes_all_layers() {
        let pipeline = PromptPipeline::default_layers();
        for layer in &pipeline.layers {
            assert!(
                layer.supports_mode(PromptMode::Full),
                "Layer '{}' should support Full mode",
                layer.name()
            );
        }
    }

    #[test]
    fn compact_mode_excludes_heavy_layers() {
        let pipeline = PromptPipeline::default_layers();
        let excluded_in_compact = [
            "runtime_context",
            "environment",
            "runtime_capabilities",
            "poe_success_criteria",
            "protocol_tokens",
            "operational_guidelines",
            "citation_standards",
            "generation_models",
            "skill_instructions",
            "special_actions",
            "guidelines",
            "thinking_guidance",
            "skill_mode",
        ];
        for layer in &pipeline.layers {
            if excluded_in_compact.contains(&layer.name()) {
                assert!(
                    !layer.supports_mode(PromptMode::Compact),
                    "Layer '{}' should NOT support Compact mode",
                    layer.name()
                );
            } else {
                assert!(
                    layer.supports_mode(PromptMode::Compact),
                    "Layer '{}' SHOULD support Compact mode",
                    layer.name()
                );
            }
        }
    }

    #[test]
    fn minimal_mode_only_core_layers() {
        let pipeline = PromptPipeline::default_layers();
        let included_in_minimal = ["soul", "tools", "hydrated_tools", "response_format", "language"];
        for layer in &pipeline.layers {
            if included_in_minimal.contains(&layer.name()) {
                assert!(
                    layer.supports_mode(PromptMode::Minimal),
                    "Layer '{}' SHOULD support Minimal mode",
                    layer.name()
                );
            } else {
                assert!(
                    !layer.supports_mode(PromptMode::Minimal),
                    "Layer '{}' should NOT support Minimal mode",
                    layer.name()
                );
            }
        }
    }

    #[test]
    fn execute_with_mode_filters_layers() {
        let pipeline = PromptPipeline::default_layers();
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);

        let full = pipeline.execute_with_mode(AssemblyPath::Basic, &input, PromptMode::Full);
        let compact = pipeline.execute_with_mode(AssemblyPath::Basic, &input, PromptMode::Compact);
        let minimal = pipeline.execute_with_mode(AssemblyPath::Basic, &input, PromptMode::Minimal);

        // Full should be longest
        assert!(full.len() > compact.len(), "Full ({}) should be longer than Compact ({})", full.len(), compact.len());
        // Compact should be longer than Minimal
        assert!(compact.len() > minimal.len(), "Compact ({}) should be longer than Minimal ({})", compact.len(), minimal.len());
        // Minimal should still have some content (response format, language)
        assert!(!minimal.is_empty(), "Minimal should not be empty");
    }
}
