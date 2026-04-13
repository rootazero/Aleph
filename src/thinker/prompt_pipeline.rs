//! PromptPipeline — composable prompt assembly engine
//!
//! The pipeline holds an ordered list of [`PromptLayer`] implementations
//! and executes them in priority order for a given [`AssemblyPath`].

use super::layers::*;
use super::prompt_budget::{enforce_budget, PromptResult, TokenBudget};
use super::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use super::prompt_mode::PromptMode;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

/// Cache hit/miss statistics for [`PromptPipeline::execute_cached`].
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub entries: usize,
}

/// Composable prompt assembly engine.
///
/// Layers are sorted by priority at construction time.  Calling
/// [`execute`](Self::execute) runs every layer whose declared paths
/// include the requested path, appending each layer's output to a
/// single `String`.
pub struct PromptPipeline {
    layers: Vec<Box<dyn PromptLayer>>,
    cache: RwLock<HashMap<&'static str, String>>,
    stats: RwLock<(u64, u64)>, // (hits, misses)
}

impl PromptPipeline {
    /// Create a new pipeline, sorting layers by ascending priority.
    pub fn new(mut layers: Vec<Box<dyn PromptLayer>>) -> Self {
        layers.sort_by_key(|l| l.priority());
        Self {
            layers,
            cache: RwLock::new(HashMap::new()),
            stats: RwLock::new((0, 0)),
        }
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

    /// Assemble system prompt with mode filtering and budget enforcement.
    ///
    /// Combines path matching, mode filtering, and total-budget enforcement
    /// into a single call.  Returns a [`PromptResult`] that includes
    /// truncation statistics when the assembled prompt exceeds the budget.
    pub fn assemble(
        &self,
        path: AssemblyPath,
        input: &LayerInput,
        mode: PromptMode,
        budget: &TokenBudget,
    ) -> PromptResult {
        // 1. Collect sections from matching layers
        let mut sections: Vec<(u32, &str, String)> = Vec::new();
        for layer in &self.layers {
            if layer.paths().contains(&path) && layer.supports_mode(mode) {
                let mut section = String::new();
                layer.inject(&mut section, input);
                if !section.is_empty() {
                    sections.push((layer.priority(), layer.name(), section));
                }
            }
        }

        // 2. Check total size
        let total: usize = sections.iter().map(|(_, _, c)| c.len()).sum();
        if total <= budget.max_total_chars {
            let prompt = sections
                .iter()
                .map(|(_, _, c)| c.as_str())
                .collect::<Vec<_>>()
                .join("");
            return PromptResult {
                prompt,
                truncation_stats: vec![],
                mode,
            };
        }

        // 3. Enforce budget — protected priorities
        let refs: Vec<(u32, &str, &str)> = sections
            .iter()
            .map(|(p, n, c)| (*p, *n, c.as_str()))
            .collect();
        let protected = &[50u32, 55, 75, 100, 500, 501, 1200];
        let (prompt, stats) = enforce_budget(&refs, budget.max_total_chars, protected);

        PromptResult {
            prompt,
            truncation_stats: stats,
            mode,
        }
    }

    /// Execute only stable layers for the given path and input.
    ///
    /// Returns the assembled string from layers whose
    /// [`stability()`](PromptLayer::stability) is [`LayerStability::Stable`].
    pub fn execute_stable_only(&self, path: AssemblyPath, input: &LayerInput) -> String {
        let mut output = String::with_capacity(16384);
        for layer in &self.layers {
            if layer.paths().contains(&path) && layer.stability() == LayerStability::Stable {
                layer.inject(&mut output, input);
            }
        }
        output
    }

    /// Execute only dynamic layers for the given path and input.
    ///
    /// Returns the assembled string from layers whose
    /// [`stability()`](PromptLayer::stability) is [`LayerStability::Dynamic`].
    pub fn execute_dynamic_only(&self, path: AssemblyPath, input: &LayerInput) -> String {
        let mut output = String::with_capacity(4096);
        for layer in &self.layers {
            if layer.paths().contains(&path) && layer.stability() == LayerStability::Dynamic {
                layer.inject(&mut output, input);
            }
        }
        output
    }

    /// Execute only stable layers with mode filtering.
    pub fn execute_stable_with_mode(
        &self,
        path: AssemblyPath,
        input: &LayerInput,
        mode: PromptMode,
    ) -> String {
        let mut output = String::with_capacity(16384);
        for layer in &self.layers {
            if layer.paths().contains(&path)
                && layer.supports_mode(mode)
                && layer.stability() == LayerStability::Stable
            {
                layer.inject(&mut output, input);
            }
        }
        output
    }

    /// Execute only dynamic layers with mode filtering.
    pub fn execute_dynamic_with_mode(
        &self,
        path: AssemblyPath,
        input: &LayerInput,
        mode: PromptMode,
    ) -> String {
        let mut output = String::with_capacity(4096);
        for layer in &self.layers {
            if layer.paths().contains(&path)
                && layer.supports_mode(mode)
                && layer.stability() == LayerStability::Dynamic
            {
                layer.inject(&mut output, input);
            }
        }
        output
    }

    /// Return `(priority, name, stability)` for each layer, sorted by priority.
    pub fn layer_info(&self) -> Vec<(u32, &'static str, LayerStability)> {
        self.layers
            .iter()
            .map(|l| (l.priority(), l.name(), l.stability()))
            .collect()
    }

    /// Create a pipeline pre-loaded with the 33 default layers.
    ///
    /// Layer order (by priority):
    ///
    /// **Stable zone** (cacheable):
    ///   50  SoulLayer
    ///   55  AgentRoleLayer
    ///   75  ProfileLayer
    ///  100  RoleLayer
    ///  300  EnvironmentLayer
    ///  400  RuntimeCapabilitiesLayer
    ///  500  ToolsLayer + HydratedToolsLayer
    ///  550  ToolUsageGrammarLayer
    ///  600  SecurityLayer
    ///  700  ProtocolTokensLayer
    ///  710  HeartbeatLayer
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
    ///
    /// **Dynamic zone** (per-request, not cacheable):
    /// 1700  InboundContextLayer
    /// 1704  AgentCatalogLayer
    /// 1705  McpInstructionsLayer
    /// 1706  McpToolIndexLayer
    /// 1710  VoiceModeLayer
    /// 1720  RuntimeContextLayer
    /// 1730  IdentityFilesLayer
    /// 1740  MemoryAugmentationLayer
    /// 1750  SessionContextGuideLayer
    /// 1760  SessionResumeLayer
    pub fn default_layers() -> Self {
        Self::new(vec![
            Box::new(SoulLayer),
            Box::new(AgentRoleLayer),
            Box::new(InboundContextLayer),
            Box::new(McpInstructionsLayer),
            Box::new(McpToolIndexLayer),
            Box::new(VoiceModeLayer),
            Box::new(ProfileLayer),
            Box::new(RoleLayer),
            Box::new(RuntimeContextLayer),
            Box::new(EnvironmentLayer),
            Box::new(RuntimeCapabilitiesLayer),
            Box::new(ToolsLayer),
            Box::new(HydratedToolsLayer),
            Box::new(AgentCatalogLayer),
            Box::new(ToolUsageGrammarLayer),
            Box::new(SecurityLayer),
            Box::new(ProtocolTokensLayer),
            Box::new(HeartbeatLayer),
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
            Box::new(IdentityFilesLayer),
            Box::new(MemoryAugmentationLayer),
            Box::new(SessionContextGuideLayer),
            Box::new(SessionResumeLayer),
            Box::new(LanguageLayer),
        ])
    }

    /// Execute with section-level caching for Stable layers.
    pub fn execute_cached(&self, path: AssemblyPath, input: &LayerInput) -> String {
        let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
        let mut output = String::with_capacity(16384);
        let mut to_cache: Vec<(&'static str, String)> = Vec::new();

        for layer in &self.layers {
            if !layer.paths().contains(&path) {
                continue;
            }

            if layer.stability() == LayerStability::Stable {
                if let Some(cached) = cache.get(layer.name()) {
                    output.push_str(cached);
                    if let Ok(mut s) = self.stats.write() {
                        s.0 += 1;
                    }
                    continue;
                }
            }

            let mut section = String::new();
            layer.inject(&mut section, input);
            if let Ok(mut s) = self.stats.write() {
                s.1 += 1;
            }

            if layer.stability() == LayerStability::Stable && !section.is_empty() {
                to_cache.push((layer.name(), section.clone()));
            }

            output.push_str(&section);
        }

        drop(cache);
        if !to_cache.is_empty() {
            if let Ok(mut w) = self.cache.write() {
                for (name, content) in to_cache {
                    w.insert(name, content);
                }
            }
        }

        output
    }

    /// Invalidate a specific layer's cache entry by name.
    pub fn invalidate(&self, layer_name: &str) {
        if let Ok(mut w) = self.cache.write() {
            w.retain(|k, _| *k != layer_name);
        }
    }

    /// Invalidate all cached sections and reset statistics.
    pub fn invalidate_all(&self) {
        if let Ok(mut w) = self.cache.write() {
            w.clear();
        }
        if let Ok(mut s) = self.stats.write() {
            *s = (0, 0);
        }
    }

    /// Cache hit/miss statistics.
    pub fn cache_stats(&self) -> CacheStats {
        let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
        let stats = self.stats.read().unwrap_or_else(|e| e.into_inner());
        CacheStats {
            hits: stats.0,
            misses: stats.1,
            entries: cache.len(),
        }
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
        fn name(&self) -> &'static str {
            self.name
        }
        fn priority(&self) -> u32 {
            self.priority
        }
        fn paths(&self) -> &'static [AssemblyPath] {
            self.paths
        }
        fn inject(&self, output: &mut String, _input: &LayerInput) {
            output.push_str(self.text);
        }
    }

    fn stub(
        name: &'static str,
        prio: u32,
        paths: &'static [AssemblyPath],
        text: &'static str,
    ) -> Box<dyn PromptLayer> {
        Box::new(StubLayer {
            name,
            priority: prio,
            paths,
            text,
        })
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
            stub("soul_only", 20, &[AssemblyPath::Soul], "SOUL"),
            stub(
                "both",
                30,
                &[AssemblyPath::Basic, AssemblyPath::Soul],
                "BOTH",
            ),
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
        assert_eq!(pipeline.layer_count(), 33);
    }

    #[test]
    fn test_default_layers_sorted() {
        let pipeline = PromptPipeline::default_layers();
        let priorities: Vec<u32> = pipeline.layers.iter().map(|l| l.priority()).collect();
        assert!(priorities.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn no_matching_path_returns_empty() {
        let pipeline =
            PromptPipeline::new(vec![stub("soul_only", 10, &[AssemblyPath::Soul], "SOUL")]);

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
            "heartbeat",
            "operational_guidelines",
            "citation_standards",
            "generation_models",
            "skill_instructions",
            "mcp_instructions",
            "mcp_tool_index",
            "agent_catalog",
            "session_resume",
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
        let included_in_minimal = [
            "soul",
            "tools",
            "hydrated_tools",
            "response_format",
            "language",
        ];
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
        assert!(
            full.len() > compact.len(),
            "Full ({}) should be longer than Compact ({})",
            full.len(),
            compact.len()
        );
        // Compact should be longer than Minimal
        assert!(
            compact.len() > minimal.len(),
            "Compact ({}) should be longer than Minimal ({})",
            compact.len(),
            minimal.len()
        );
        // Minimal should still have some content (response format, language)
        assert!(!minimal.is_empty(), "Minimal should not be empty");
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    use crate::thinker::prompt_budget::TokenBudget;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_mode::PromptMode;

    #[test]
    fn assemble_with_budget_trims_when_over() {
        let pipeline = PromptPipeline::default_layers();
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);

        // First, get the full prompt size so we know what budget to set
        let full_result = pipeline.assemble(
            AssemblyPath::Basic,
            &input,
            PromptMode::Full,
            &TokenBudget {
                max_total_chars: 500_000,
                ..Default::default()
            },
        );
        let full_len = full_result.prompt.len();

        // Budget smaller than full output, but large enough to keep protected layers
        let budget = TokenBudget {
            max_total_chars: full_len / 2,
            ..Default::default()
        };

        let result = pipeline.assemble(AssemblyPath::Basic, &input, PromptMode::Full, &budget);
        // Some sections should have been removed
        assert!(
            !result.truncation_stats.is_empty(),
            "Should have truncation stats"
        );
        // Prompt should be smaller than full
        assert!(
            result.prompt.len() < full_len,
            "Trimmed prompt ({}) should be smaller than full ({})",
            result.prompt.len(),
            full_len
        );
    }

    #[test]
    fn assemble_under_budget_no_truncation() {
        let pipeline = PromptPipeline::default_layers();
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);

        // Large budget — nothing should be trimmed
        let budget = TokenBudget {
            max_total_chars: 500_000,
            ..Default::default()
        };

        let result = pipeline.assemble(AssemblyPath::Basic, &input, PromptMode::Full, &budget);
        assert!(
            result.truncation_stats.is_empty(),
            "Should have no truncation stats"
        );
        assert_eq!(result.mode, PromptMode::Full);
    }

    #[test]
    fn full_pipeline_mode_and_budget_integration() {
        let pipeline = PromptPipeline::default_layers();
        let config = PromptConfig::default();
        let tools = vec![];
        let budget = TokenBudget::default();

        let input_full = LayerInput::basic(&config, &tools).with_mode(PromptMode::Full);
        let input_compact = LayerInput::basic(&config, &tools).with_mode(PromptMode::Compact);
        let input_minimal = LayerInput::basic(&config, &tools).with_mode(PromptMode::Minimal);

        let full = pipeline.assemble(AssemblyPath::Basic, &input_full, PromptMode::Full, &budget);
        let compact = pipeline.assemble(
            AssemblyPath::Basic,
            &input_compact,
            PromptMode::Compact,
            &budget,
        );
        let minimal = pipeline.assemble(
            AssemblyPath::Basic,
            &input_minimal,
            PromptMode::Minimal,
            &budget,
        );

        // Full > Compact > Minimal
        assert!(
            full.prompt.len() > compact.prompt.len(),
            "Full ({}) > Compact ({})",
            full.prompt.len(),
            compact.prompt.len()
        );
        assert!(
            compact.prompt.len() > minimal.prompt.len(),
            "Compact ({}) > Minimal ({})",
            compact.prompt.len(),
            minimal.prompt.len()
        );

        // All should have no truncation (default budget is 80K)
        assert!(full.truncation_stats.is_empty());
        assert!(compact.truncation_stats.is_empty());
        assert!(minimal.truncation_stats.is_empty());

        // Modes are correctly recorded
        assert_eq!(full.mode, PromptMode::Full);
        assert_eq!(compact.mode, PromptMode::Compact);
        assert_eq!(minimal.mode, PromptMode::Minimal);

        // Minimal should still have content (response format at minimum)
        assert!(!minimal.prompt.is_empty());
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn cached_execute_returns_same_result() {
        let pipeline = PromptPipeline::default_layers();
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let r1 = pipeline.execute_cached(AssemblyPath::Basic, &input);
        let r2 = pipeline.execute_cached(AssemblyPath::Basic, &input);
        assert_eq!(r1, r2);
    }

    #[test]
    fn invalidate_clears_cache() {
        let pipeline = PromptPipeline::default_layers();
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let _ = pipeline.execute_cached(AssemblyPath::Basic, &input);
        pipeline.invalidate_all();
        assert_eq!(pipeline.cache_stats().entries, 0);
    }

    #[test]
    fn second_call_hits_cache() {
        let pipeline = PromptPipeline::default_layers();
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let _ = pipeline.execute_cached(AssemblyPath::Basic, &input);
        let s1 = pipeline.cache_stats();
        let _ = pipeline.execute_cached(AssemblyPath::Basic, &input);
        let s2 = pipeline.cache_stats();
        assert!(s2.hits > s1.hits, "Second call should have cache hits");
    }
}

#[cfg(test)]
mod stability_tests {
    use super::*;
    use crate::thinker::prompt_layer::LayerStability;

    #[test]
    fn stable_layers_come_before_dynamic() {
        let pipeline = PromptPipeline::default_layers();
        let layers = pipeline.layer_info();

        let mut found_dynamic = false;
        for (priority, name, stability) in &layers {
            if *stability == LayerStability::Dynamic {
                found_dynamic = true;
            }
            if found_dynamic && *stability == LayerStability::Stable {
                panic!(
                    "Stable layer '{}' (priority {}) found after dynamic layer",
                    name, priority
                );
            }
        }
        // Ensure we actually found both stable and dynamic layers
        let stable_count = layers
            .iter()
            .filter(|(_, _, s)| *s == LayerStability::Stable)
            .count();
        let dynamic_count = layers
            .iter()
            .filter(|(_, _, s)| *s == LayerStability::Dynamic)
            .count();
        assert!(stable_count > 0, "Should have stable layers");
        assert!(dynamic_count > 0, "Should have dynamic layers");
    }

    #[test]
    fn dynamic_layers_are_correctly_classified() {
        let pipeline = PromptPipeline::default_layers();
        let dynamic_names: Vec<&str> = pipeline
            .layer_info()
            .into_iter()
            .filter(|(_, _, s)| *s == LayerStability::Dynamic)
            .map(|(_, n, _)| n)
            .collect();

        assert!(dynamic_names.contains(&"inbound_context"));
        assert!(dynamic_names.contains(&"voice_mode"));
        assert!(dynamic_names.contains(&"runtime_context"));
        assert!(dynamic_names.contains(&"identity_files"));
        assert!(dynamic_names.contains(&"memory_augmentation"));
        assert!(dynamic_names.contains(&"session_context_guide"));
        assert!(dynamic_names.contains(&"session_resume"));
        assert!(dynamic_names.contains(&"mcp_instructions"));
        assert!(dynamic_names.contains(&"mcp_tool_index"));
        assert!(dynamic_names.contains(&"agent_catalog"));
        assert_eq!(
            dynamic_names.len(),
            10,
            "Exactly 10 dynamic layers expected"
        );
    }

    #[test]
    fn execute_stable_only_excludes_dynamic() {
        let pipeline = PromptPipeline::default_layers();
        let config = crate::thinker::prompt_builder::PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);

        let stable = pipeline.execute_stable_only(AssemblyPath::Basic, &input);
        let dynamic = pipeline.execute_dynamic_only(AssemblyPath::Basic, &input);
        let full = pipeline.execute(AssemblyPath::Basic, &input);

        // stable + dynamic should reconstruct the full output
        let combined = format!("{}{}", stable, dynamic);
        assert_eq!(combined, full);
    }

    #[test]
    fn layer_info_returns_sorted_entries() {
        let pipeline = PromptPipeline::default_layers();
        let info = pipeline.layer_info();
        let priorities: Vec<u32> = info.iter().map(|(p, _, _)| *p).collect();
        assert!(priorities.windows(2).all(|w| w[0] <= w[1]));
    }
}
