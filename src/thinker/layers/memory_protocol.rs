//! MemoryProtocolLayer — soft guidance for the three memory tools (priority 1745)
//!
//! Sits between `MemoryAugmentationLayer` (1740, hybrid-retrieval injection) and
//! `SessionContextGuideLayer` (1750, post-compaction guide). Always-on, stable
//! text — the LLM's view of which memory tool fits which question.
//!
//! Spec A introduced `remember` (curated MEMORY.md hot zone). Spec B introduced
//! `session_search` (summarized session-end facts). Without explicit guidance
//! the model often only reaches for `memory_search`, missing the lighter and
//! more recent layers. P3 adds nudges, not rules — the harness must keep its
//! LLM-sovereignty stance (CLAUDE.md R8).
//!
//! Why this is a separate layer rather than text glued onto an existing one:
//! * `MemoryAugmentationLayer` injects retrieved content verbatim — mixing
//!   guidance into it would couple "what we know" with "how to recall more".
//! * `SessionContextGuideLayer` only fires after compaction. Tool guidance
//!   must apply to the first turn too.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct MemoryProtocolLayer;

impl PromptLayer for MemoryProtocolLayer {
    fn name(&self) -> &'static str {
        "memory_protocol"
    }

    fn priority(&self) -> u32 {
        1745
    }

    fn stability(&self) -> LayerStability {
        // Convention in `prompt_pipeline.rs`: priority ≥ 1700 belongs in the
        // dynamic zone, which is enforced by `stable_layers_come_before_dynamic`.
        // The text is identical across requests, but priority 1745 keeps the
        // guidance visually adjacent to `MemoryAugmentationLayer` (1740) so it
        // reads as one block — that ordering wins over the marginal cache
        // benefit a Stable rating would give. Sibling memory/context layers
        // (MemoryAugmentationLayer / SessionContextGuideLayer) make the same
        // call.
        LayerStability::Dynamic
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        !matches!(mode, PromptMode::Minimal)
    }

    fn inject(&self, output: &mut String, _input: &LayerInput) {
        output.push_str(
            "\n\n## Memory Protocol\n\
             You have three memory tools. Reach for the one that matches the question:\n\
             - `memory_search` — hybrid retrieval over notes/facts (cross-session). \
             Use when the user asks about prior decisions, preferences, or any fact \
             you do not already see in `<CuratedMemory>` or `<memory>` blocks above.\n\
             - `session_search` — find a past session by topic and read its summarized \
             facts (with evidence quotes). Use when the user references \"last time\", \
             \"that bug we fixed\", or any past-conversation context.\n\
             - `remember` — append/replace/remove the curated MEMORY.md hot zone. \
             Use proactively when you learn a stable user preference, environment fact, \
             or a correction the user wants you to honor next session. Do not store \
             task progress, completed-work logs, or transient TODOs.\n\
             \n\
             What is already visible: `<CuratedMemory>` and any retrieved `<memory>` \
             blocks are auto-injected — do not search for facts you can already read. \
             Soft rejections from `remember` (duplicate, over-budget, no-match) come \
             back as a tool result with `message: \"rejected: …\"`; recover by \
             rephrasing or switching action, not by aborting the turn.\n",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn metadata() {
        let layer = MemoryProtocolLayer;
        assert_eq!(layer.name(), "memory_protocol");
        assert_eq!(layer.priority(), 1745);
        // Dynamic so the priority-zone convention holds (≥1700 = dynamic).
        assert_eq!(layer.stability(), LayerStability::Dynamic);
        for path in [
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ] {
            assert!(layer.paths().contains(&path), "missing path {path:?}");
        }
    }

    #[test]
    fn supports_full_and_compact_not_minimal() {
        let layer = MemoryProtocolLayer;
        assert!(layer.supports_mode(PromptMode::Full));
        assert!(layer.supports_mode(PromptMode::Compact));
        assert!(!layer.supports_mode(PromptMode::Minimal));
    }

    #[test]
    fn injects_three_tool_names() {
        let layer = MemoryProtocolLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("memory_search"));
        assert!(out.contains("session_search"));
        assert!(out.contains("remember"));
        assert!(out.contains("Memory Protocol"));
    }

    #[test]
    fn mentions_already_visible_blocks() {
        // Regression — if we forget to tell the LLM that CuratedMemory is
        // already in the prompt, it'll waste tool calls re-searching for it.
        let layer = MemoryProtocolLayer;
        let config = PromptConfig::default();
        let mut out = String::new();
        layer.inject(&mut out, &LayerInput::basic(&config, &[]));
        assert!(out.contains("CuratedMemory"));
        assert!(out.contains("auto-injected"));
    }

    #[test]
    fn mentions_soft_rejection_recovery() {
        // P2 + P3 connection — the LLM must know that a soft rejection from
        // `remember` is recoverable, not a hard error.
        let layer = MemoryProtocolLayer;
        let config = PromptConfig::default();
        let mut out = String::new();
        layer.inject(&mut out, &LayerInput::basic(&config, &[]));
        assert!(out.contains("rejected:"));
        assert!(out.contains("rephrasing") || out.contains("recover"));
    }

    #[test]
    fn always_injects_regardless_of_input_flags() {
        // P3 contract — guidance is always-on, not conditional like
        // SessionContextGuideLayer (which only fires after compaction).
        let layer = MemoryProtocolLayer;
        let config = PromptConfig::default();
        let mut out_no_session = String::new();
        layer.inject(&mut out_no_session, &LayerInput::basic(&config, &[]));
        let mut out_with_session = String::new();
        layer.inject(
            &mut out_with_session,
            &LayerInput::basic(&config, &[]).with_session_summaries(true),
        );
        assert!(!out_no_session.is_empty());
        assert_eq!(out_no_session, out_with_session, "text must not vary");
    }
}
