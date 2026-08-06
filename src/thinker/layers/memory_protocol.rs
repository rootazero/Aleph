//! `MemoryProtocolLayer` — the cross-tool memory destination ladder (priority 1105)
//!
//! Sits beside `SpecialActionsLayer` (1100) — the other layer whose whole job is
//! ranking tools against each other — and inside the **cacheable prefix**: every
//! byte it renders is a compile-time constant.
//!
//! **The volatile half is a different layer.** [`MemoryWindowLayer`] (1745,
//! Dynamic) carries the one sentence that varies per turn: the claim that
//! `<CuratedMemory>` / `<memory-context>` are already in this request's window.
//! The two were a single Dynamic layer until 2026-08-03, which dragged the
//! constant ladder into the dynamic tail with it — where it was re-written at
//! 1.25x every time a genuinely volatile neighbour moved, because
//! `split_system_blocks_for_cache` stamps `cache_control` on the stable block
//! only (FEATURE_LOCATOR §2.18 ledger item 10; same tax that moved
//! `agent_catalog`, `identity_files` and `extra_files` out). The split moves
//! those bytes without deleting a sentence.
//!
//! Worth recording because the measurement inverted the intuition: under
//! `prompt_contract`'s production-shaped input **both** window-claim gates are
//! false, so all of the layer's measured bytes were the constant, and none were
//! the part that justified the Dynamic rating.
//!
//! **Scope rule — this layer carries only what no single tool can state.** That
//! is the destination ladder that ranks `remember` / `flag_user_correction` /
//! `note_manage` / scratchpad against each other. Everything else about a memory
//! tool — what it does, what its actions mean, how to recover from a soft
//! rejection, how to acknowledge a write — belongs in that tool's own
//! `DESCRIPTION`, which ships with its schema on every request that can call it.
//! (Retrieved memory itself does not ride the system prompt at all: it arrives
//! as the transient trailing `<memory-context>` message, see
//! `HarnessDeps::recall_context`.)
//!
//! This layer was ~1,150 tokens on 2026-07-26 and roughly two thirds of it was a
//! second copy of `RememberTool::DESCRIPTION` (hot-tier framing, demote-when-
//! full, the D4 acknowledgment contract) — some of it verbatim. Restating a
//! tool's own docs in the always-on prompt costs those tokens on every single
//! request and creates a second place for the rule to drift. Trimmed to the
//! cross-tool core; the displaced sentences now live once, in the tools (pi's
//! rule: tool semantics live with the tool).
//!
//! Why this is a separate layer rather than text glued onto an existing one:
//! * `SessionContextGuideLayer` only fires after compaction. Routing guidance
//!   must apply to the first turn too.
//!
//! [`MemoryWindowLayer`]: super::MemoryWindowLayer

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct MemoryProtocolLayer;

impl PromptLayer for MemoryProtocolLayer {
    fn name(&self) -> &'static str {
        "memory_protocol"
    }

    fn priority(&self) -> u32 {
        1105
    }

    fn stability(&self) -> LayerStability {
        // Stable because the bytes below are a `&'static str` — no input reaches
        // them, which `ladder_is_a_constant` asserts directly rather than by
        // inspection.
        //
        // It was Dynamic until 2026-08-03, and the reason recorded here has been
        // wrong twice in a row — worth keeping, because the second version was
        // cited as precedent by another layer and so was propagating:
        //
        //   v1: *"The text is identical across requests, so a Dynamic rating
        //   costs no provider-cache stability (byte-identical dynamic bytes
        //   never re-key the prefix)."* — the premise was right for the ladder
        //   and the conclusion still wrong. The dynamic system block carries no
        //   `cache_control` marker of its own; it is covered only by the
        //   message-level breakpoints, and those all sit *after* it. Unchanged
        //   bytes parked there do not *cause* a miss but still *pay* for one:
        //   whenever any other dynamic layer moves, the whole dynamic block is
        //   re-written at 1.25x.
        //
        //   v2: "Dynamic because the window claim varies per turn" — true of the
        //   window claim, and it was the whole justification for a layer that
        //   was ~1,037 B of constant plus ~200 B of claim. A layer is classified
        //   as one thing; when its halves disagree, the answer is two layers,
        //   not the worse of the two ratings.
        //
        // The rule that survives: `stability()` states whether the content
        // varies. Priority states reading order. Deciding one from the other —
        // in either direction — is how layers end up in the wrong zone.
        LayerStability::Stable
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        !matches!(mode, PromptMode::Minimal)
    }

    fn inject(&self, output: &mut String, _input: &LayerInput) {
        output.push_str(
            "\n\n## Memory Protocol\n\
             Where a NEW memory goes — ONE destination ladder, first matching rung wins:\n\
             1. Durable user preference / identity fact / standing instruction → `remember` \
             (HOT tier: MEMORY.md, always in-prompt, tiny).\n\
             2. You made a mistake and the user corrected you → `flag_user_correction`. \
             Do NOT hand-write `feedback/` notes for corrections — the distillation gate \
             deduplicates and strengthens them. Self-discovered lessons with no user \
             correction go to `note_manage` as a `lesson` note.\n\
             3. Reusable domain knowledge / how-to / project facts worth retrieving later → \
             `note_manage` (DURABLE tier: searchable notes DB, recalled on relevance).\n\
             4. Transient task state / plan → scratchpad, never a memory tool.\n\
             Prefer UPDATE over CREATE: when an existing entry or note already covers the \
             topic, `replace`/`append`/`update` it instead of adding a near-duplicate.\n\
             \n\
             Reading back: `memory_search` for a prior decision or fact; `session_search` \
             when the user points at a past conversation (\"last time\", \"that bug we \
             fixed\").\n",
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
        // Beside `special_actions` (1100), the other cross-tool routing layer,
        // and below 1700 so it rides the cacheable prefix.
        assert_eq!(layer.priority(), 1105);
        assert!(
            layer.priority() < 1700,
            "constant bytes belong in the prefix"
        );
        assert_eq!(layer.stability(), LayerStability::Stable);
        for path in [AssemblyPath::Basic, AssemblyPath::Cached] {
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
    fn routes_between_the_read_side_tools() {
        // Choosing *between* two read tools is cross-tool, so it stays here;
        // what each one does is the tool's own description's job.
        let layer = MemoryProtocolLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("Memory Protocol"));
        assert!(out.contains("memory_search"));
        assert!(out.contains("session_search"));
    }

    #[test]
    fn ladder_is_a_constant() {
        // This is what earns the `Stable` rating, so it is asserted rather than
        // argued: no combination of inputs — including the two that used to
        // vary this layer, before the window claim moved to `MemoryWindowLayer`
        // — changes a byte. A layer whose bytes move under some input belongs in
        // the dynamic tail, and `dynamic_tail_bytes_ratchet` then charges for it.
        let layer = MemoryProtocolLayer;
        let config = PromptConfig::default();
        let render = |input: &LayerInput| {
            let mut out = String::new();
            layer.inject(&mut out, input);
            out
        };

        let baseline = render(&LayerInput::basic(&config, &[]));
        assert!(!baseline.is_empty());
        for input in [
            LayerInput::basic(&config, &[]).with_session_summaries(true),
            LayerInput::basic(&config, &[])
                .with_curated_envelope(Some("<CuratedMemory>x</CuratedMemory>".to_string())),
            LayerInput::basic(&config, &[]).with_recalled_memory(true),
            LayerInput::basic(&config, &[])
                .with_curated_envelope(Some("<CuratedMemory>x</CuratedMemory>".to_string()))
                .with_recalled_memory(true),
        ] {
            assert_eq!(render(&input), baseline, "ladder must not vary with input");
        }
    }

    #[test]
    fn window_claim_moved_out() {
        // The split is load-bearing for the dynamic-tail ceiling, and a revert
        // would be invisible: re-inlining the claim here still renders correct
        // prompt text, it just puts ~1 KB of constant back in the 1.25x zone.
        let layer = MemoryProtocolLayer;
        let config = PromptConfig::default();
        let mut out = String::new();
        layer.inject(
            &mut out,
            &LayerInput::basic(&config, &[])
                .with_curated_envelope(Some("<CuratedMemory>x</CuratedMemory>".to_string()))
                .with_recalled_memory(true),
        );
        assert!(!out.contains("already in this prompt"), "{out}");
        assert!(!out.contains("already in this conversation"), "{out}");
        assert!(
            !out.contains("don't search for facts you can already read"),
            "{out}"
        );
    }

    #[test]
    fn per_tool_how_to_lives_in_the_tools_single_home() {
        // The scope rule, enforced. Each of these sentences is about ONE tool,
        // so it belongs in that tool's `DESCRIPTION` (shipped with its schema)
        // and must not be duplicated into the always-on prompt. Both halves are
        // asserted: absent here, present there.
        use crate::tools::AlephTool;
        let layer = MemoryProtocolLayer;
        let config = PromptConfig::default();
        let mut out = String::new();
        layer.inject(&mut out, &LayerInput::basic(&config, &[]));

        // `remember`: declarative-vs-imperative phrasing (D1), soft-rejection
        // recovery, demote-when-full, and the acknowledgment contract (D4).
        assert!(!out.contains("declarative fact"));
        assert!(!out.contains("rejected:"));
        assert!(!out.contains("demote"));
        assert!(!out.contains("Acknowledgment contract"));
        let remember = <crate::builtin_tools::RememberTool as AlephTool>::DESCRIPTION;
        assert!(remember.contains("declarative fact"));
        assert!(remember.contains("never as an imperative"));
        assert!(remember.contains("rejected:") && remember.contains("rephrasing"));
        assert!(remember.contains("DEMOTE the least-hot entry"));
        assert!(remember.contains("one short sentence") && remember.contains("user's language"));

        // `note_manage`: same D4 acknowledgment contract, its own copy.
        let notes = <crate::builtin_tools::note_manage::NoteManageTool as AlephTool>::DESCRIPTION;
        assert!(notes.contains("one short sentence") && notes.contains("user's language"));
        assert!(notes.contains("Never quote the stored content back verbatim"));
    }

    #[test]
    fn injects_destination_ladder() {
        // D1 — ONE authoritative destination ladder. Ranking four destinations
        // against each other is the thing no single tool's description can say,
        // so this is the layer's core payload: all four rungs plus
        // update-over-create. (The overflow valve — demote the least-hot entry
        // when MEMORY.md is full — is `remember`-only and lives in that tool's
        // description; `remember`'s own tests pin it.)
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

    /// The three writing tools of the destination ladder — the set the D4
    /// contract is defined over. Kept as one list so the two halves below
    /// cannot drift apart into "asserted for some tools".
    fn ladder_writer_descriptions() -> [&'static str; 3] {
        use crate::tools::AlephTool;
        [
            <crate::builtin_tools::RememberTool as AlephTool>::DESCRIPTION,
            <crate::builtin_tools::note_manage::NoteManageTool as AlephTool>::DESCRIPTION,
            <crate::builtin_tools::FlagUserCorrectionTool as AlephTool>::DESCRIPTION,
        ]
    }

    #[test]
    fn acknowledgment_contract_is_stated_once_per_writing_tool() {
        // D4 survives the trim, but as one statement per writing tool instead
        // of a fourth copy in the always-on prompt. All three writers must
        // carry it — a tool that drops it silently drops the contract for its
        // own writes, which no amount of prompt prose would restore.
        for desc in ladder_writer_descriptions() {
            let lower = desc.to_lowercase();
            assert!(
                lower.contains("one short sentence"),
                "missing the one-sentence ack: {desc}"
            );
            assert!(
                lower.contains("user's language") || lower.contains("their language"),
                "ack must be in the user's language: {desc}"
            );
            assert!(
                lower.contains("do not quote") || lower.contains("never quote"),
                "ack must not echo stored content: {desc}"
            );
        }
    }

    #[test]
    fn the_refusal_half_of_the_contract_is_stated_too() {
        // D4 had one half. Every one of these tools can settle without
        // writing anything — an over-budget hot zone, a spent retry budget, a
        // correction already on record — and each returns a SUCCESSFUL tool
        // result to say so. Stating only "acknowledge after a successful
        // write" leaves the model with no instruction for the case where the
        // user asked for something durable and the system declined: it either
        // reports a save that never happened, or goes quiet, and a silent
        // skip is not evidence that nothing was asked for.
        //
        // Asserted over the same list as the positive half, in one loop, so
        // the two cannot end up covering different tools.
        for desc in ladder_writer_descriptions() {
            assert!(
                desc.contains("never acknowledge a save that did not happen")
                    || desc.contains("Never acknowledge a save that did not happen"),
                "missing the refusal-side contract: {desc}"
            );
        }
    }
}
