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
             proactively for a stable preference, environment fact, or standing \
             instruction to honor next session; not for task progress, work logs, or \
             transient TODOs. \
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
             Where a NEW memory goes — ONE destination ladder, first matching rung wins:\n\
             1. Durable user preference / identity fact / standing instruction → `remember` \
             (HOT tier: MEMORY.md, always in-prompt, tiny). A few identity-level facts \
             re-read every session: who the user is, stable preferences, environment quirks.\n\
             2. You made a mistake and the user corrected you → `flag_user_correction` \
             (severity-tagged; flushed immediately, distilled into a `feedback/` note by \
             the nightly dream cycle). Do NOT hand-write `feedback/` notes for corrections — \
             the distillation gate deduplicates and strengthens them. Self-discovered \
             lessons with no user correction go to `note_manage` as a `lesson` note.\n\
             3. Reusable domain knowledge / how-to / project facts worth retrieving later → \
             `note_manage` (DURABLE tier: searchable notes DB, recalled on relevance, \
             organized by category).\n\
             4. Transient task state / plan → scratchpad, never a memory tool.\n\
             Prefer UPDATE over CREATE: when an existing entry or note already covers the \
             topic, `replace`/`append`/`update` it instead of adding a near-duplicate. When \
             the hot zone is full, demote the least-hot entry to a note, then `remove` it \
             from MEMORY.md — preserve the knowledge, free the hot space.\n\
             \n\
             Acknowledgment contract: after a successful memory write (`remember`, \
             `flag_user_correction`, `note_manage`), tell the user in ONE short sentence, \
             in their language, what was recorded and to which tier — use the destination \
             info from the tool result. Never quote the stored content back verbatim, and \
             treat the tool's success response as terminal: do not repeat the write or \
             re-echo the entry into another memory tool call.\n",
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
    fn injects_destination_ladder() {
        // D1 — ONE authoritative destination ladder replaces the old two-tier
        // split (and the three-way competing guidance across remember's
        // description, special_actions and this layer). The guidance must
        // spell out all four rungs, update-over-create, and the
        // overflow-valve (demote-to-note) recovery.
        let layer = MemoryProtocolLayer;
        let config = PromptConfig::default();
        let mut out = String::new();
        layer.inject(&mut out, &LayerInput::basic(&config, &[]));
        assert!(
            out.contains("ONE destination ladder"),
            "must declare the single authoritative ladder"
        );
        assert!(out.contains("HOT") && out.contains("DURABLE"));
        assert!(out.contains("MEMORY.md") && out.contains("notes DB"));
        assert!(
            out.contains("flag_user_correction") && out.contains("scratchpad"),
            "all four rungs must be present"
        );
        assert!(
            out.contains("Prefer UPDATE over CREATE"),
            "must prefer update over near-duplicate adds"
        );
        // Overflow valve: full hot zone demotes to a note rather than dropping it.
        assert!(out.contains("demote"));
    }

    #[test]
    fn corrections_route_through_the_distillation_gate() {
        // D3 — the old nudge steered user-corrected mistakes to direct
        // `note_manage` feedback/ writes, bypassing the FeedbackDistill
        // dedupe/strengthen gate. Corrections must route through
        // `flag_user_correction`; only self-discovered lessons go to
        // `note_manage` (as `lesson` notes, not `feedback/`).
        let layer = MemoryProtocolLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(
            !out.contains("feedback/lessons"),
            "direct feedback/ hand-write nudge must be gone"
        );
        assert!(out.contains("Do NOT hand-write `feedback/` notes"));
        assert!(out.contains("note_manage"));
    }

    #[test]
    fn injects_acknowledgment_contract() {
        // D4 — after a successful memory write the model owes the user one
        // short sentence naming what was recorded and to which tier, and must
        // treat the tool's success response as terminal (anti-thrash: models
        // re-echoing entries have caused duplicate write storms).
        let layer = MemoryProtocolLayer;
        let config = PromptConfig::default();
        let mut out = String::new();
        layer.inject(&mut out, &LayerInput::basic(&config, &[]));
        assert!(out.contains("Acknowledgment contract"));
        assert!(out.contains("ONE short sentence") && out.contains("their language"));
        assert!(
            out.contains("terminal"),
            "success response must be terminal"
        );
        assert!(out.contains("Never quote the stored content back verbatim"));
    }
}
