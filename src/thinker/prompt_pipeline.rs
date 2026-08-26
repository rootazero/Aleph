//! `PromptPipeline` — composable prompt assembly engine
//!
//! The pipeline holds an ordered list of [`PromptLayer`] implementations
//! and executes them in priority order for a given [`AssemblyPath`].

use super::layers::{
    AgentCatalogLayer, AgentRoleLayer, ChainContextLayer, CitationStandardsLayer,
    CuratedMemoryLayer, DoctorRepairHintLayer, EnvironmentLayer, ExecutionPlanLayer,
    ExtraFilesLayer, GraphTopologyLayer, GuidelinesLayer, IdentityFilesLayer, LanguageLayer,
    McpInstructionsLayer, MemoryProtocolLayer, MemoryWindowLayer, MultiStepConductLayer,
    OperatingEnvelopeLayer, OperationalGuidelinesLayer, ProfileLayer, ProtocolTokensLayer,
    ProviderGuidanceLayer, RoleLayer, RoomRosterLayer, RuntimeCapabilitiesLayer,
    RuntimeContextLayer, SecurityLayer, SessionBudgetLayer, SessionContextGuideLayer,
    SkillInstructionsLayer, SoulLayer, SpecialActionsLayer, StandingGoalLayer, StrategyLayer,
    StrategyPointerLayer, TimerLoopLayer, ToolRuntimeStateLayer, VoiceModeLayer,
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

    /// Names of registered layers that contribute **nothing** for this
    /// `(path, mode, input)` — the complement of [`Self::layer_breakdown`].
    ///
    /// A layer lands here for one of two reasons, and the distinction matters:
    /// it is filtered out by the path / mode, or it ran and its input gate was
    /// unset. Either way the operator's question is the same ("why isn't my
    /// section in the prompt?"), and a breakdown that only lists winners cannot
    /// answer it. Silent-by-omission is the failure mode that has cost this
    /// module the most (`Context` / `Hydration` / `Soul` phantom paths, four
    /// layers whose `PromptConfig` gate had no production writer), so the
    /// diagnostic reports the silent set explicitly.
    pub fn silent_layers(
        &self,
        path: AssemblyPath,
        input: &LayerInput,
        mode: PromptMode,
    ) -> Vec<&'static str> {
        let mut out = Vec::new();
        let mut section = String::new();
        for layer in &self.layers {
            if !layer.paths().contains(&path) || !layer.supports_mode(mode) {
                out.push(layer.name());
                continue;
            }
            section.clear();
            layer.inject(&mut section, input);
            if section.is_empty() {
                out.push(layer.name());
            }
        }
        out
    }

    /// Per-layer rendered text for this `(path, mode, input)`, in assembly
    /// order, skipping layers that emit nothing.
    ///
    /// The text twin of [`Self::layer_breakdown`], which keeps only sizes.
    /// Used by the cross-layer duplicate-sentence guard, which needs to know
    /// *which* layer said a thing, not just how big it was.
    #[cfg(test)]
    pub(crate) fn layer_sections(
        &self,
        path: AssemblyPath,
        input: &LayerInput,
        mode: PromptMode,
    ) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        for layer in &self.layers {
            if layer.paths().contains(&path) && layer.supports_mode(mode) {
                let mut section = String::new();
                layer.inject(&mut section, input);
                if !section.is_empty() {
                    out.push((layer.name(), section));
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
            // @105 — who else is in this room. Silent (zero bytes) for every
            // session that is not a project room.
            Box::new(RoomRosterLayer),
            Box::new(RuntimeContextLayer),
            Box::new(EnvironmentLayer),
            Box::new(RuntimeCapabilitiesLayer),
            // ToolsLayer @500 was removed (2026-07-26): it injected tool
            // names + JSON schemas as prompt text for providers without native
            // `tool_use`. Both production writers force
            // `native_tools_enabled = true`, so it always early-returned — and
            // the other half of that protocol (the `{reasoning, action}`
            // text-envelope parser) was deleted on 2026-05-10, so a prompt-only
            // revival could not have worked anyway. `native_tools_enabled` went
            // with it. (It also named a phantom tool, `search_tools`; the real
            // discovery tool is `tool_search`.)
            Box::new(ToolRuntimeStateLayer),
            Box::new(AgentCatalogLayer),
            // ToolUsageGrammarLayer @550 was removed (2026-07-26): it rendered
            // `ToolInfo::usage_hint` preferences, and `usage_hint` had zero
            // producers repo-wide — every production `ToolInfo` sets it to
            // `None`. It also read `input.tools`, which the production cached
            // path leaves empty (native tool_use delivers schemas out-of-band),
            // so the layer was dead twice over. Its one live sentence (parallel
            // tool calls) moved to `RoleLayer`, which always fires.
            Box::new(SecurityLayer),
            Box::new(ProtocolTokensLayer),
            Box::new(OperationalGuidelinesLayer),
            Box::new(MultiStepConductLayer),
            Box::new(ProviderGuidanceLayer),
            Box::new(SessionBudgetLayer),
            Box::new(CitationStandardsLayer),
            // GenerationModelsLayer @910 was removed (2026-07-26):
            // `PromptConfig.generation_models` had no production writer, so the
            // layer only ever rendered inside its own unit test. Media-model
            // discovery reaches the model through the `list_models` tool.
            Box::new(SkillInstructionsLayer),
            Box::new(SpecialActionsLayer),
            // @1105, beside `special_actions` — the two layers that rank tools
            // against each other. It was @1745/Dynamic until 2026-08-03; see
            // `MemoryWindowLayer` below for why the pair split.
            Box::new(MemoryProtocolLayer),
            Box::new(DoctorRepairHintLayer),
            // ResponseFormatLayer was removed (2026-05-10..06-08): it mandated
            // the legacy `{reasoning, action}` JSON envelope, which had no live
            // consumer once the harness moved to native `with_tools(...)`.
            Box::new(GuidelinesLayer),
            // ThinkingGuidanceLayer @1350 was removed (2026-07-26):
            // `PromptConfig.thinking_transparency` had no production writer.
            // CustomInstructionsLayer @1500 went with it — its own body called
            // itself a "legacy fallback" that yields to IDENTITY.md, and
            // `initialize_agent_identity` always writes IDENTITY.md, so the
            // fallback branch was unreachable even if the field were fed.
            Box::new(IdentityFilesLayer),
            Box::new(ExtraFilesLayer),
            // MemoryAugmentationLayer (@1740, Dynamic) was removed 2026-07-03:
            // per-query recall now travels as a transient trailing user
            // message (`HarnessDeps::recall_context`) instead of the system
            // prompt — varying bytes here re-keyed the conversation-prefix
            // cache every run. Curated memory stays at @60 (Stable).
            //
            // `MemoryProtocolLayer` used to sit here too, at @1745/Dynamic. It
            // carried two things with opposite cache profiles — a constant
            // destination ladder and the per-turn window claim below — and was
            // rated by the volatile half, so ~1,037 B of never-changing text
            // rode the unmarked dynamic block. The ladder moved to @1105/Stable
            // and only the claim stayed. A layer is classified as one thing;
            // when its halves disagree, the answer is two layers.
            Box::new(MemoryWindowLayer),
            Box::new(SessionContextGuideLayer),
            Box::new(TimerLoopLayer),
            Box::new(GraphTopologyLayer),
            Box::new(StandingGoalLayer),
            Box::new(ExecutionPlanLayer),
            Box::new(StrategyPointerLayer),
            Box::new(OperatingEnvelopeLayer),
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
        fn stability(&self) -> LayerStability {
            LayerStability::Stable
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
            stub("cached_only", 20, &[AssemblyPath::Cached], "CACHED"),
            stub(
                "both",
                30,
                &[AssemblyPath::Basic, AssemblyPath::Cached],
                "BOTH",
            ),
        ]);

        let config = PromptConfig::default();
        let tools: Vec<crate::tools::info::ToolInfo> = vec![];
        let input = LayerInput::basic(&config, &tools);

        let basic_result = pipeline.execute(AssemblyPath::Basic, &input);
        assert_eq!(basic_result, "BASICBOTH");

        let cached_result = pipeline.execute(AssemblyPath::Cached, &input);
        assert_eq!(cached_result, "CACHEDBOTH");
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
        // GraphTopologyLayer retained (41 @1754 Dynamic, right after
        // TimerLoopLayer @1753 — tells a governed
        // session its place in the loop-graph governance topology,
        // 2026-07-19).
        // → 36 (2026-07-26, Pi leanness round): four layers deleted because no
        // production input can make them speak — ToolUsageGrammarLayer
        // (`usage_hint` has zero producers), ThinkingGuidanceLayer
        // (`thinking_transparency` never set), GenerationModelsLayer
        // (`generation_models` never set), CustomInstructionsLayer
        // (`custom_instructions` never set AND self-declared legacy fallback
        // that IDENTITY.md always pre-empts). `reachable_layers` below is the
        // structural guard that stops this class from recurring.
        // → 35: ToolsLayer too — both writers force `native_tools_enabled =
        // true`, and the text-envelope parser that would have consumed its
        // prompt-injected tool listings was deleted 2026-05-10.
        // → 36 (OperatingEnvelopeLayer @1758 Dynamic, 2026-07-26): the approval
        // tier and usage mode moved OUT of `SecurityLayer` @600, which is Stable
        // by default — so flipping a composer pill mid-conversation rewrote a byte
        // in the cacheable prefix and invalidated the whole conversation's prompt
        // cache. Net prompt bytes unchanged; only the cache zone differs.
        // → 37 (MemoryWindowLayer @1745 Dynamic, 2026-08-03): `memory_protocol`
        // split in two. It carried a constant destination ladder AND a per-turn
        // window claim, and a layer gets one `stability()` for both — so the
        // whole thing was rated Dynamic and ~1,037 B of never-changing text rode
        // the system block that carries no `cache_control` marker of its own.
        // The ladder stayed `memory_protocol` and moved to @1105/Stable; only
        // the claim is still Dynamic. Net prompt bytes unchanged, and the
        // dynamic-layer COUNT unchanged too (one left, one arrived) — the whole
        // effect is in `prompt_contract::dynamic_tail_bytes_ratchet`.
        // → 38 (RoomRosterLayer @105 Stable, 2026-08-25): a project room's
        // member list. NOT a split of an existing layer — a genuinely new
        // runtime fact, and the first prompt section that exists only for
        // multi-human sessions. Silent for every personal or org session, so
        // a single-human deployment's prompt is byte-identical; declared in
        // `prompt_contract::CONDITIONALLY_SILENT` with the content that wakes
        // it.
        assert_eq!(pipeline.layer_count(), 38);
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
            PromptPipeline::new(vec![stub("cached_only", 10, &[AssemblyPath::Cached], "C")]);

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
            "skill_instructions",
            "mcp_instructions",
            "agent_catalog",
            "chain_context",
            "special_actions",
            "multi_step_conduct",
            "guidelines",
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
        // `tools` dropped 2026-07-26 with ToolsLayer. Every survivor here is
        // input-gated (identity file / curated envelope / configured language),
        // so a Minimal prompt built from a bare config is legitimately empty.
        let included_in_minimal = ["soul", "curated_memory", "language"];
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
        // Give the one always-available Minimal layer something to render, so
        // the ordering assertion below measures mode filtering rather than the
        // emptiness of a bare config.
        let config = PromptConfig {
            language: Some("zh-Hans".to_string()),
            ..PromptConfig::default()
        };
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
        // Minimal keeps the configured response language and nothing else.
        assert!(minimal.contains("Chinese (Simplified)"));
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
        // `memory_window` is the per-turn half of what used to be one
        // `memory_protocol` layer: the claim that `<CuratedMemory>` /
        // `<memory-context>` are already in this request's window, chosen from
        // `(curated_block_present, has_recalled_memory)`. The constant
        // destination ladder is NOT here — it moved to @1105/Stable, which is
        // the whole point of the split.
        assert!(dynamic_names.contains(&"memory_window"));
        assert!(
            !dynamic_names.contains(&"memory_protocol"),
            "the constant ladder is back in the 1.25x zone"
        );
        assert!(dynamic_names.contains(&"session_context_guide"));
        assert!(dynamic_names.contains(&"mcp_instructions"));
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
        // `extra_files` and `identity_files` used to be asserted here, on the
        // reasoning that content "re-read off disk per prompt build" is dynamic.
        // That reads the wrong property: re-reading a file that has not changed
        // produces the same bytes, and this classification is about whether the
        // bytes vary — see the note on the count below.
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
        // → 16: GraphTopologyLayer (@1754, one slot after TimerLoopLayer @1753) —
        // session-scoped governance topology. It carries no clock, but the `graph`
        // tool can add or retire an edge mid-session, so the rows do vary within a
        // session. (This comment used to say the classification was "about content
        // ownership, not volatility" — see the → 14 note below for why that
        // reasoning was retired.)
        assert!(dynamic_names.contains(&"graph_topology"));
        // → 17: OperatingEnvelopeLayer (@1758) — the approval tier and usage mode
        // are per-turn pills, so they must NOT sit in the Stable cacheable prefix
        // where `SecurityLayer` @600 used to render them. It also took the sandbox
        // `Writable roots` line (2026-08-03): an isolated run mints a worktree path
        // with a fresh UUID, so rendering it from Stable meant no two isolated runs
        // could share a prefix at all.
        assert!(dynamic_names.contains(&"operating_envelope"));
        // → 14 (2026-08-03, FEATURE_LOCATOR §2.18 ledger item 10): `agent_catalog`,
        // `identity_files` and `extra_files` moved to the Stable zone.
        //
        // This is the round that fixed the *criterion*, not just three layers.
        // Classification had drifted to "priority ≥ 1700 belongs to the dynamic
        // zone" plus reasons like content ownership or being re-read off disk —
        // neither of which is about whether the bytes change. The cost was real:
        // the dynamic system block gets no `cache_control` marker of its own
        // (`split_system_blocks_for_cache` stamps only the stable block), so it is
        // covered solely by message-level breakpoints that all sit after it.
        // Session-stable content parked here does not *cause* a cache miss, but it
        // is re-written at 1.25x whenever a genuinely volatile neighbour moves —
        // and `identity_files` alone can render 100 000 chars by default.
        //
        // The criterion is now: **does this layer's content vary within a
        // session?** Yes → Dynamic. No → Stable, and give it a priority below
        // 1700. `prompt_contract::dynamic_tail_bytes_ratchet` holds the line.
        //
        // → 14 again (same day, follow-up): `memory_protocol` left this zone and
        // `memory_window` took its place, so the COUNT did not move while ~1 KB
        // did. That is the case this number cannot see, and the reason the two
        // `contains` assertions above are spelled out by name: applying the
        // criterion above to a layer whose halves disagree does not produce a
        // reclassification, it produces a split. Total layer count therefore
        // grew by one while this stayed put — see `test_default_layers_count`.
        assert_eq!(
            dynamic_names.len(),
            14,
            "Exactly 14 dynamic layers expected"
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
