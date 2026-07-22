//! `PromptPipeline` — composable prompt assembly engine
//!
//! The pipeline holds an ordered list of [`PromptLayer`] implementations
//! and executes them in priority order for a given [`AssemblyPath`].

use super::layers::{
    AgentCatalogLayer, AgentRoleLayer, ChainContextLayer, CitationStandardsLayer,
    CuratedMemoryLayer, CustomInstructionsLayer, DoctorRepairHintLayer, EnvironmentLayer,
    ExecutionPlanLayer, ExtraFilesLayer, GenerationModelsLayer, GraphTopologyLayer,
    GuidelinesLayer, IdentityFilesLayer, LanguageLayer, McpInstructionsLayer,
    MemoryProtocolLayer, MultiStepConductLayer, OperationalGuidelinesLayer, ProfileLayer,
    ProtocolTokensLayer, ProviderGuidanceLayer, RoleLayer, RuntimeCapabilitiesLayer,
    RuntimeContextLayer, SecurityLayer, SessionBudgetLayer, SessionContextGuideLayer,
    SkillInstructionsLayer, SoulLayer, SpecialActionsLayer, StandingGoalLayer, StrategyLayer,
    StrategyPointerLayer, ThinkingGuidanceLayer, TimerLoopLayer, ToolRuntimeStateLayer,
    ToolUsageGrammarLayer, ToolsLayer, VoiceModeLayer,
};
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
/// Cross-build reuse of the stable prefix is provided by the cached two-part
/// split (`PromptBuilder::build_system_prompt_cached_with_mode`), which emits
/// the Stable layers as one cacheable [`SystemPromptPart`] and the Dynamic
/// layers as a marker-free tail — not by an internal name-keyed cache. A fresh
/// builder is constructed per prompt build, so an in-pipeline cache never
/// served a cross-call hit and could only ever return stale sections if a
/// builder were reused. The pipeline therefore holds no mutable cache state.
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
    /// WITHOUT budget truncation — the introspection twin of the (pruned) `assemble`.
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
        let mut section = String::new();
        for layer in &self.layers {
            if layer.paths().contains(&path) && layer.supports_mode(mode) {
                section.clear();
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
            Box::new(ChainContextLayer),
            Box::new(McpInstructionsLayer),
            Box::new(VoiceModeLayer),
            Box::new(ProfileLayer),
            Box::new(RoleLayer),
            Box::new(RuntimeContextLayer),
            Box::new(EnvironmentLayer),
            Box::new(RuntimeCapabilitiesLayer),
            Box::new(ToolsLayer),
            Box::new(ToolRuntimeStateLayer),
            Box::new(AgentCatalogLayer),
            Box::new(ToolUsageGrammarLayer),
            Box::new(SecurityLayer),
            Box::new(ProtocolTokensLayer),
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
            Box::new(GraphTopologyLayer),
            Box::new(StandingGoalLayer),
            Box::new(ExecutionPlanLayer),
            Box::new(StrategyPointerLayer),
            // SessionResumeLayer (@1760, Dynamic) was removed 2026-07-17: its
            // only input (`LayerInput::session_snapshot`) was never set outside
            // the layer's own unit test — the prior-session snapshot actually
            // reaches the model through the assembler's memory envelope
            // (`SnapshotReader` → `HybridAssembler::fetch_snapshot`).
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
        // → 42: HydratedToolsLayer and SkillModeLayer deleted. Both were
        // unreachable — HydratedToolsLayer's only `paths()` entry was the
        // Hydration assembly path, fed by a `HydrationPipeline` with zero
        // callers, and SkillModeLayer's gate was never true outside tests (it
        // also mandated a legacy JSON tool envelope contradicting the native
        // tool_use contract we actually ship).
        // → 40: InboundContextLayer and SessionResumeLayer deleted — both were
        // registered but their `LayerInput` fields (inbound / session_snapshot)
        // were never threaded on any production path, so they injected nothing
        // every run. (Session resume actually reaches the model via the memory
        // assembler's recall message, not the system prompt.)
        // → 41: McpResourceIndexLayer (1704, Dynamic) added, then → 40: removed
        // (2026-07-17). It eagerly injected a resource/prompt catalog, but the
        // ids it rendered carried a single `server:` prefix, while the read path
        // strips two symmetric layers — so a copied index id did NOT round-trip
        // through `mcp_read_resource`. Discovery converged on the on-demand
        // `mcp_list_resources`/`mcp_list_prompts` tools (doubled, verbatim ids
        // that do round-trip; R7/R10 static partition, no prompt-layer — as
        // MODEL_PERCEIVABLE_ECOSYSTEM.md already declared).
        // → 40 (was 41): HeartbeatLayer removed (§1.1 prune-the-prompt —
        // a pure progress-reporting how-to with `[step N/total]` template,
        // redundant with MultiStepConductLayer's narration guidance).
        // GraphTopologyLayer retained (41 @1753 Dynamic — tells a governed
        // session its place in the loop-graph governance topology,
        // 2026-07-19).
        assert_eq!(pipeline.layer_count(), 40);
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
            "protocol_tokens",
            "operational_guidelines",
            "provider_guidance",
            "session_budget",
            "citation_standards",
            "generation_models",
            "skill_instructions",
            "mcp_instructions",
            "agent_catalog",
            "chain_context",
            "special_actions",
            "multi_step_conduct",
            "guidelines",
            "thinking_guidance",
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
        let included_in_minimal = ["soul", "curated_memory", "tools", "language"];
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
mod stability_tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::LayerStability;
    use crate::thinker::prompt_mode::PromptMode;

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

        assert!(dynamic_names.contains(&"voice_mode"));
        assert!(dynamic_names.contains(&"runtime_context"));
        assert!(dynamic_names.contains(&"identity_files"));
        assert!(dynamic_names.contains(&"memory_protocol"));
        assert!(dynamic_names.contains(&"session_context_guide"));
        assert!(dynamic_names.contains(&"mcp_instructions"));
        assert!(dynamic_names.contains(&"agent_catalog"));
        // Live tool health is per-request state. Classified Stable (by omission)
        // at priority 502, it rode the cached prefix and let a 30s-TTL probe flip
        // invalidate the entire conversation's prompt cache.
        assert!(dynamic_names.contains(&"tool_runtime_state"));
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
        // 16 → 15 → 16: MemoryAugmentationLayer removed 2026-07-03 (recall
        // moved to the transient trailing message, `HarnessDeps::recall_context`)
        // while MemoryProtocolLayer (Dynamic since creation) landed from the
        // other side of merge b8ea7a7c8 — the two sides were individually
        // consistent, so this count silently drifted to 15 vs 16 actual.
        // → 17: ToolRuntimeStateLayer moved from 502/Stable to 1702/Dynamic.
        // It renders a snapshot of the 30s-TTL, mutable tool-health cache, so
        // riding the CACHED prefix meant one MCP probe flip rewrote that prefix
        // mid-session and invalidated the whole conversation's prompt cache. It
        // is per-request state and now sits in the per-request zone.
        // → 15: InboundContextLayer (1700) and SessionResumeLayer (1760), both
        // Dynamic, were deleted as dead injection surfaces (their LayerInput
        // fields were never threaded in production).
        // → 16: McpResourceIndexLayer (1704, Dynamic) added, then → 15: removed
        // (2026-07-17) — its eager resource/prompt index emitted single-prefix
        // ids that did not round-trip through the two-strip read path; discovery
        // converged on the on-demand mcp_list_resources/mcp_list_prompts tools.
        // → 16: GraphTopologyLayer (@1753) — session-scoped governance
        // topology; deterministic bytes (graph rows only, no clocks), so the
        // Dynamic classification is about content ownership, not volatility.
        assert!(dynamic_names.contains(&"graph_topology"));
        // Every name above is asserted individually; the count pins the set.
        assert_eq!(
            dynamic_names.len(),
            16,
            "Exactly 16 dynamic layers expected"
        );
    }

    #[test]
    fn stable_plus_dynamic_reconstructs_full() {
        // The stable/dynamic split is the prompt-cache breakpoint: the two
        // halves must concatenate back to the full assembly with no overlap or
        // gap. Asserted on the LIVE `_with_mode` methods that the cached
        // production path (`build_system_prompt_cached_with_mode`) actually
        // uses.
        let pipeline = PromptPipeline::default_layers();
        let config = crate::thinker::prompt_builder::PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);

        let stable =
            pipeline.execute_stable_with_mode(AssemblyPath::Basic, &input, PromptMode::Full);
        let dynamic =
            pipeline.execute_dynamic_with_mode(AssemblyPath::Basic, &input, PromptMode::Full);
        let full = pipeline.execute_with_mode(AssemblyPath::Basic, &input, PromptMode::Full);

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
    fn default_layers_have_unique_priorities() {
        use std::collections::HashMap;
        let pipeline = PromptPipeline::default_layers();
        let mut seen: HashMap<u32, &str> = HashMap::new();
        for layer in &pipeline.layers {
            if let Some(prev) = seen.insert(layer.priority(), layer.name()) {
                panic!(
                    "priority {} is shared by '{}' and '{}'",
                    layer.priority(),
                    prev,
                    layer.name()
                );
            }
        }
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
