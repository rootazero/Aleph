//! `MemoryProtocolLayer` — soft guidance for the three memory tools (priority 1745)
//!
//! Sits just before `SessionContextGuideLayer` (1750, post-compaction guide).
//! Always-on, stable text — the LLM's view of which memory tool fits which
//! question. (Retrieved memory itself no longer rides the system prompt: it
//! arrives as the transient trailing `<memory-context>` message, see
//! `HarnessDeps::recall_context`.)
//!
//! Spec A introduced `remember` (curated MEMORY.md hot zone). Spec B introduced
//! `session_search` (summarized session-end facts). Without explicit guidance
//! the model often only reaches for `memory_search`, missing the lighter and
//! more recent layers. P3 adds nudges, not rules — the harness must keep its
//! LLM-sovereignty stance (CLAUDE.md R8).
//!
//! Why this is a separate layer rather than text glued onto an existing one:
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
        // The text is identical across requests, so a Dynamic rating costs no
        // provider-cache stability (byte-identical dynamic bytes never re-key
        // the prefix); priority 1745 keeps the guidance adjacent to the other
        // per-request memory/session context. `SessionContextGuideLayer` makes
        // the same call.
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
             Three memory tools — reach for the one matching the question:\n\
             - `memory_search` — hybrid retrieval over notes/facts (cross-session). \
             Use for prior decisions, preferences, or any fact not already in the \
             `<CuratedMemory>` block above or the auto-recalled `<memory-context>` \
             message in this conversation.\n\
             - `session_search` — find a past session by topic and read its summarized \
             facts (with evidence quotes). Use for \"last time\", \"that bug we fixed\", \
             or any past-conversation reference.\n\
             - `remember` — append/replace/remove the curated MEMORY.md hot zone. Use \
             proactively for a stable preference, environment fact, or correction to \
             honor next session; not for task progress, work logs, or transient TODOs. \
             Phrase each entry as a declarative fact about the user or environment \
             (\"User prefers X\"), not an imperative to yourself (\"Always do X\") — \
             imperatives get re-read next session as standing orders and can override \
             a later request.\n\
             \n\
             `<CuratedMemory>` and the retrieved `<memory-context>` message are \
             auto-injected — don't \
             search for facts you can already read. A soft rejection from `remember` \
             (duplicate, over-budget, no-match) returns `message: \"rejected: …\"`; \
             recover by rephrasing or switching action, not by aborting the turn.\n\
             \n\
             Where a NEW memory goes — two write tiers, pick by how often it's needed:\n\
             - HOT (always in-prompt, tiny) → `remember` → MEMORY.md. A few identity-level \
             facts re-read every session: who the user is, stable preferences, environment \
             quirks, standing corrections.\n\
             - DURABLE (searchable, recalled on relevance) → `note_manage` → notes DB. \
             Everything else worth keeping: project facts, learnings, references, lessons. \
             Larger, organized by category, surfaced only when a query matches.\n\
             Rule of thumb: want this in front of you EVERY session regardless of topic? → \
             `remember`. Only when the topic comes up? → `note_manage`. When the hot zone is \
             full, demote the least-hot entry to a note, then `remove` it from MEMORY.md — \
             preserve the knowledge, free the hot space.\n\
             \n\
             When you recognize a mistake — your own or one the user corrected — \
             record the lesson immediately with `note_manage` (create a \
             `feedback/lessons` note): state the cause (why it happened) and how to \
             avoid it next time. Don't wait for the session to end; a durable lesson \
             written now is recalled (and linked) for the next session.\n",
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
        // Pass-3: `remember` entries must be phrased as declarative facts, not
        // imperatives — an imperative re-read next session becomes a standing
        // order that can override the user's current request.
        assert!(out.contains("declarative fact"));
        assert!(out.contains("imperative"));
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

    #[test]
    fn injects_write_routing_two_tiers() {
        // The user's core question — when recording a long-term memory, how does
        // the agent distinguish the hot identity file (remember/MEMORY.md) from
        // the durable searchable DB (note_manage)? The guidance must spell out
        // both tiers and the overflow-valve (demote-to-note) recovery.
        let layer = MemoryProtocolLayer;
        let config = PromptConfig::default();
        let mut out = String::new();
        layer.inject(&mut out, &LayerInput::basic(&config, &[]));
        assert!(out.contains("two write tiers"), "must name the two tiers");
        assert!(out.contains("HOT") && out.contains("DURABLE"));
        assert!(out.contains("MEMORY.md") && out.contains("notes DB"));
        // Overflow valve: full hot zone demotes to a note rather than dropping it.
        assert!(out.contains("demote"));
    }

    #[test]
    fn injects_lesson_capture_nudge() {
        let layer = MemoryProtocolLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(
            out.contains("feedback/lessons"),
            "must teach immediate lesson capture on error"
        );
        assert!(out.contains("note_manage"));
    }
}
