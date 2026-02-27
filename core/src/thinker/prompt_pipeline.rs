//! PromptPipeline — composable prompt assembly engine
//!
//! The pipeline holds an ordered list of [`PromptLayer`] implementations
//! and executes them in priority order for a given [`AssemblyPath`].

use super::layers::*;
use super::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

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

    /// Create a pipeline pre-loaded with the 21 default layers.
    ///
    /// Layer order (by priority):
    ///   50  SoulLayer
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
        assert_eq!(pipeline.layer_count(), 21);
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
