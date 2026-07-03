//! `PromptPipeline` — composable prompt assembly engine
//!
//! The pipeline holds an ordered list of [`PromptLayer`] implementations
//! and executes them in priority order for a given [`AssemblyPath`].

use super::layers::{
    AgentCatalogLayer, AgentRoleLayer, ChainContextLayer, CitationStandardsLayer,
    CuratedMemoryLayer, CustomInstructionsLayer, DoctorRepairHintLayer, EnvironmentLayer,
    ExecutionPlanLayer, ExtraFilesLayer, GenerationModelsLayer, GuidelinesLayer, HeartbeatLayer,
    HydratedToolsLayer, IdentityFilesLayer, InboundContextLayer, LanguageLayer,
    McpInstructionsLayer, MemoryProtocolLayer, MultiStepConductLayer, OperationalGuidelinesLayer,
    ProfileLayer, ProtocolTokensLayer, ProviderGuidanceLayer, RoleLayer, RuntimeCapabilitiesLayer,
    RuntimeContextLayer, SecurityLayer, SessionBudgetLayer, SessionContextGuideLayer,
    SessionResumeLayer, SkillInstructionsLayer, SkillModeLayer, SoulLayer, SpecialActionsLayer,
    StandingGoalLayer, StrategyLayer, StrategyPointerLayer, ThinkingGuidanceLayer, TimerLoopLayer,
    ToolRuntimeStateLayer, ToolUsageGrammarLayer, ToolsLayer, VoiceModeLayer,
};
use super::prompt_budget::{enforce_budget, PromptResult, TokenBudget};
use super::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use super::prompt_mode::PromptMode;
use crate::context::budget::pressure::{estimate_tokens_aware, DEFAULT_PROSE_RATIO};

/// One layer's contribution to the assembled prompt, for size introspection
/// and budget tuning. Produced by [`PromptPipeline::layer_breakdown`].
#[derive(Debug, Clone)]
pub struct LayerSize {
    pub priority: u32,
    pub name: &'static str,
    pub stability: LayerStability,
    /// Unicode scalar count of the layer's emitted section.
    pub chars: usize,
    /// UTF-8 byte length of the layer's emitted section. The on-the-wire size
    /// (hermes `prompt_size.py` reports bytes); differs from `chars` for any
    /// multi-byte content (CJK, emoji).
    pub bytes: usize,
    /// Content-aware token estimate (CJK/code denser than prose).
    pub tokens: usize,
}

/// Composable prompt assembly engine.
///
/// Layers are sorted by priority at construction time.  Calling
/// [`execute`](Self::execute) runs every layer whose declared paths
/// include the requested path, appending each layer's output to a
/// single `String`.
///
/// Cross-build reuse of the stable prefix is provided by the explicit,
/// input-keyed snapshot path ([`execute_stable_only`](Self::execute_stable_only)
/// / `PromptBuilder::capture_snapshot`) and the cached two-part split
/// (`PromptBuilder::build_system_prompt_cached_with_mode`), not by an internal
/// name-keyed cache — a fresh builder is constructed per prompt build, so an
/// in-pipeline cache never served a cross-call hit and could only ever return
/// stale sections if a builder were reused. The pipeline therefore holds no
/// mutable cache state.
pub struct PromptPipeline {
    layers: Vec<Box<dyn PromptLayer>>,
}

impl PromptPipeline {
    /// Create a new pipeline, sorting layers by ascending priority.
    #[must_use]
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

        // 2. Check total size. Budget is in *characters* (`max_total_chars`),
        //    so measure characters — a 3-byte CJK glyph counts as one unit,
        //    matching the char-accurate truncation in `prompt_budget`. (ASCII
        //    is byte==char, so this is identical for English-only prompts.)
        let total: usize = sections.iter().map(|(_, _, c)| c.chars().count()).sum();
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

        // 3. Enforce budget — never trim the foundational layers. Each entry
        //    maps to a real registered layer (see `default_layers`): 50 soul,
        //    55 agent_role, 60 curated_memory, 75 profile, 100 role, 500 tools,
        //    501 hydrated_tools, 600 security. The previous list carried a
        //    phantom `1200` (no layer at that priority) and left `security`
        //    (600) trimmable — safety guidance must survive budget pressure.
        let refs: Vec<(u32, &str, &str)> = sections
            .iter()
            .map(|(p, n, c)| (*p, *n, c.as_str()))
            .collect();
        let protected = &[50u32, 55, 60, 75, 100, 500, 501, 600];
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

    /// Measure each active layer's contribution for the given path/mode/input,
    /// WITHOUT budget truncation — the introspection twin of [`assemble`].
    ///
    /// Mirrors `assemble`'s section-collection phase (same path + mode filter,
    /// same `!section.is_empty()` skip) but keeps the per-layer sizes that
    /// `assemble` discards. Use it to see which layers dominate the prompt
    /// before tuning their content or mode gating. Layers are already stored in
    /// priority order, so the result is assembly order.
    pub fn layer_breakdown(
        &self,
        path: AssemblyPath,
        input: &LayerInput,
        mode: PromptMode,
    ) -> Vec<LayerSize> {
        let mut out = Vec::new();
        for layer in &self.layers {
            if layer.paths().contains(&path) && layer.supports_mode(mode) {
                let mut section = String::new();
                layer.inject(&mut section, input);
                if !section.is_empty() {
                    out.push(LayerSize {
                        priority: layer.priority(),
                        name: layer.name(),
                        stability: layer.stability(),
                        chars: section.chars().count(),
                        bytes: section.len(),
                        // Canonical prose anchor; CJK/code auto-densify.
                        tokens: estimate_tokens_aware(&section, DEFAULT_PROSE_RATIO),
                    });
                }
            }
        }
        out
    }

    /// Create a pipeline pre-loaded with the default layer set.
    ///
    /// Each layer self-declares its `priority()` (ascending assembly order) and
    /// its `stability()`: the cacheable **Stable** prefix zone (priorities
    /// `< 1700`: persona, tools, security, skills …) followed by the
    /// per-request **Dynamic** suffix zone (`>= 1700`: inbound/session/memory/
    /// runtime context). The registration block below is the authoritative
    /// ordering — a hand-maintained priority table was removed because it
    /// silently drifted as layers were added. Invariants are locked by tests:
    /// `test_default_layers_count` (exact layer count),
    /// `test_default_layers_sorted` (priority monotonicity), and
    /// `stable_layers_come_before_dynamic` (the Stable→Dynamic split that
    /// anchors the Anthropic prefix-cache breakpoint).
    #[must_use]
    pub fn default_layers() -> Self {
        Self::new(vec![
            Box::new(SoulLayer),
            Box::new(AgentRoleLayer),
            Box::new(CuratedMemoryLayer),
            Box::new(StrategyLayer),
            Box::new(InboundContextLayer),
            Box::new(ChainContextLayer),
            Box::new(McpInstructionsLayer),
            Box::new(VoiceModeLayer),
            Box::new(ProfileLayer),
            Box::new(RoleLayer),
            Box::new(RuntimeContextLayer),
            Box::new(EnvironmentLayer),
            Box::new(RuntimeCapabilitiesLayer),
            Box::new(ToolsLayer),
            Box::new(HydratedToolsLayer),
            Box::new(ToolRuntimeStateLayer),
            Box::new(AgentCatalogLayer),
            Box::new(ToolUsageGrammarLayer),
            Box::new(SecurityLayer),
            Box::new(ProtocolTokensLayer),
            Box::new(HeartbeatLayer),
            Box::new(OperationalGuidelinesLayer),
            Box::new(MultiStepConductLayer),
            Box::new(ProviderGuidanceLayer),
            Box::new(SessionBudgetLayer),
            Box::new(CitationStandardsLayer),
            Box::new(GenerationModelsLayer),
            Box::new(SkillInstructionsLayer),
            Box::new(SpecialActionsLayer),
            Box::new(DoctorRepairHintLayer),
            // ResponseFormatLayer was removed (2026-05-10..06-08): it mandated
            // the legacy `{reasoning, action}` JSON envelope, which had no live
            // consumer once the harness moved to native `with_tools(...)`.
            Box::new(GuidelinesLayer),
            Box::new(ThinkingGuidanceLayer),
            Box::new(SkillModeLayer),
            Box::new(CustomInstructionsLayer),
            Box::new(IdentityFilesLayer),
            Box::new(ExtraFilesLayer),
            // MemoryAugmentationLayer (@1740, Dynamic) was removed 2026-07-03:
            // per-query recall now travels as a transient trailing user
            // message (`HarnessDeps::recall_context`) instead of the system
            // prompt — varying bytes here re-keyed the conversation-prefix
            // cache every run. Curated memory stays at @60 (Stable).
            Box::new(MemoryProtocolLayer),
            Box::new(SessionContextGuideLayer),
            Box::new(TimerLoopLayer),
            Box::new(StandingGoalLayer),
            Box::new(ExecutionPlanLayer),
            Box::new(StrategyPointerLayer),
            Box::new(SessionResumeLayer),
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
        let tools: Vec<crate::tools::info::ToolInfo> = vec![];
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
        let tools: Vec<crate::tools::info::ToolInfo> = vec![];
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
        let tools: Vec<crate::tools::info::ToolInfo> = vec![];
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
        // 40 after ExtraFilesLayer was added (2026-06-10).
        // Previous counts: 36 → 37 (SessionBudgetLayer, Phase 4, 2026-05-20)
        // → 38 (ToolRuntimeStateLayer) → 37 (McpToolIndexLayer removed as dead
        // code, 2026-05-31) → 38 (ExecutionPlanLayer re-surfaces the active
        // scratchpad plan per turn) → 39 (StandingGoalLayer re-surfaces the
        // active standing goal per turn, 2026-06-08) → 40 (ExtraFilesLayer
        // renders `[prompt.extra_files]`, 2026-06-10). See `default_layers`.
        // → 42 (StrategyLayer @70 Stable + StrategyPointerLayer @1756 Dynamic
        // weld the StraTA plan into the cacheable head + per-turn tail,
        // 2026-06-18). See `default_layers`.
        // → 43 (DoctorRepairHintLayer @1715 — WebRich-only "/doctor → press f"
        // hint, 2026-06-19). See `default_layers`.
        // → 44 (MultiStepConductLayer @805 Stable — autonomous scratchpad
        // planning + interactive progress narration, 2026-06-28).
        // → 43 (MemoryAugmentationLayer removed 2026-07-03 — per-query recall
        // moved out of the system prompt into the transient trailing recall
        // message; see `HarnessDeps::recall_context`).
        // → 44 (TimerLoopLayer @1753 Dynamic — re-surfaces the session's
        // active watch loop per turn, 2026-07-03).
        assert_eq!(pipeline.layer_count(), 44);
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
        let tools: Vec<crate::tools::info::ToolInfo> = vec![];
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
            "provider_guidance",
            "session_budget",
            "citation_standards",
            "generation_models",
            "skill_instructions",
            "mcp_instructions",
            "agent_catalog",
            "chain_context",
            "session_resume",
            "special_actions",
            "multi_step_conduct",
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
        // 2026-05-10: `response_format` removed (ResponseFormatLayer is no
        // longer registered). `curated_memory` added to align with reality —
        // CuratedMemoryLayer (commit a89af2844) inherits the default
        // `supports_mode = true`, so it implicitly participates in Minimal.
        let included_in_minimal = [
            "soul",
            "curated_memory",
            "tools",
            "hydrated_tools",
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
mod stability_tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;
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
        assert!(dynamic_names.contains(&"memory_protocol"));
        assert!(dynamic_names.contains(&"session_context_guide"));
        assert!(dynamic_names.contains(&"session_resume"));
        assert!(dynamic_names.contains(&"mcp_instructions"));
        assert!(dynamic_names.contains(&"agent_catalog"));
        // Phase 2 (2026-05-20): ChainContextLayer rendered subagent
        // delegation depth per request — naturally dynamic.
        assert!(dynamic_names.contains(&"chain_context"));
        // ExecutionPlanLayer re-surfaces the active scratchpad plan per turn.
        assert!(dynamic_names.contains(&"execution_plan"));
        // StandingGoalLayer re-surfaces the active standing goal per turn.
        assert!(dynamic_names.contains(&"standing_goal"));
        // TimerLoopLayer re-surfaces the active watch loop per turn.
        assert!(dynamic_names.contains(&"timer_loop"));
        // ExtraFilesLayer renders user-configured `[prompt.extra_files]`
        // content, re-read off disk per prompt build — naturally dynamic.
        assert!(dynamic_names.contains(&"extra_files"));
        // StrategyPointerLayer echoes the Strategy guardrails near the read
        // head per turn — Dynamic. (StrategyLayer @70 is Stable, not counted.)
        assert!(dynamic_names.contains(&"strategy_pointer"));
        // DoctorRepairHintLayer (@1715) is WebRich-gated per request — Dynamic.
        assert!(dynamic_names.contains(&"doctor_repair_hint"));
        // 16 → 15: MemoryAugmentationLayer removed 2026-07-03 (recall moved
        // to the transient trailing message, `HarnessDeps::recall_context`).
        assert_eq!(
            dynamic_names.len(),
            15,
            "Exactly 15 dynamic layers expected"
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

    #[test]
    fn layer_breakdown_sums_to_assembled_prompt() {
        let pipeline = PromptPipeline::default_layers();
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);

        let breakdown = pipeline.layer_breakdown(AssemblyPath::Basic, &input, PromptMode::Full);
        assert!(!breakdown.is_empty(), "breakdown should list active layers");

        // Priority order (assembly order).
        let priorities: Vec<u32> = breakdown.iter().map(|l| l.priority).collect();
        assert!(
            priorities.windows(2).all(|w| w[0] <= w[1]),
            "breakdown must be in priority order"
        );

        // Every listed layer emitted something; token estimate is non-zero
        // whenever there are chars.
        for l in &breakdown {
            assert!(l.chars > 0, "layer {} listed with zero chars", l.name);
            assert!(l.tokens > 0, "layer {} has chars but zero tokens", l.name);
            assert!(l.bytes >= l.chars, "layer {} bytes < chars", l.name);
        }

        // The per-layer char counts must account for the whole assembled
        // (untrimmed) prompt — same path/mode, generous budget so nothing trims.
        let assembled = pipeline.execute_with_mode(AssemblyPath::Basic, &input, PromptMode::Full);
        let sum_chars: usize = breakdown.iter().map(|l| l.chars).sum();
        assert_eq!(
            sum_chars,
            assembled.chars().count(),
            "sum of per-layer chars must equal the assembled prompt length"
        );
        // Same invariant on the byte axis.
        let sum_bytes: usize = breakdown.iter().map(|l| l.bytes).sum();
        assert_eq!(
            sum_bytes,
            assembled.len(),
            "sum of per-layer bytes must equal the assembled prompt byte length"
        );
    }
}
