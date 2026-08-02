//! `MemoryProtocolLayer` — cross-tool memory routing (priority 1745)
//!
//! Sits just before `SessionContextGuideLayer` (1750, post-compaction guide).
//! Always-on routing; only the window claim below varies per turn. (Retrieved
//! memory itself no longer rides the system prompt: it arrives as the transient
//! trailing `<memory-context>` message, see `HarnessDeps::recall_context`.)
//!
//! **Scope rule — this layer carries only what no single tool can state.** That
//! is exactly two things: the destination ladder that ranks `remember` /
//! `flag_user_correction` / `note_manage` / scratchpad against each other, and
//! the runtime fact that memory is already in the window (so searching for it
//! wastes a turn). The ladder is unconditional — cross-tool routing applies in
//! every configuration — but the window claim is a statement of runtime fact,
//! and it names two blocks with two different producers, so each half is
//! emitted only when that turn's input proves its own block is there.
//! Everything else about a memory tool — what it does, what its
//! actions mean, how to recover from a soft rejection, how to acknowledge a
//! write — belongs in that tool's own `DESCRIPTION`, which ships with its
//! schema on every request that can call it.
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

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct MemoryProtocolLayer;

/// Whether `<CuratedMemory>` really lands in *this* request's system prompt.
///
/// Deliberately the same predicate `CuratedMemoryLayer::inject` (priority 60)
/// applies to the same field — `Some` plus a non-blank body. Re-deriving it any
/// other way is how the description and the described block drift apart.
fn curated_block_present(input: &LayerInput) -> bool {
    input
        .curated_memory_envelope
        .as_ref()
        .is_some_and(|text| !text.trim().is_empty())
}

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
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        !matches!(mode, PromptMode::Minimal)
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        output.push_str("\n\n## Memory Protocol\n");
        // The window claim is a statement of runtime fact, and it names two
        // blocks that two DIFFERENT producers decide independently — so it is
        // two claims, each gated on its own producer. Merging them (or gating
        // both on one flag) reintroduces the bug: the sentence asserts a block
        // that is not there, the model doesn't search for what it cannot read,
        // and the only recovery is another turn.
        //
        // * `<CuratedMemory>` ← `build_curated_message`, which does NOT gate on
        //   `MemoryInjectionMode`. Provable here from the input, using the
        //   exact predicate `CuratedMemoryLayer` (priority 60) applies to the
        //   same field, so the claim and the block can never disagree.
        // * `<memory-context>` ← `build_memory_user_message`, which returns
        //   `Ok(None)` under `MemoryInjectionMode::Tools` (the mode whose whole
        //   point is retrieval-by-tool) and for an empty query. It rides the
        //   transient tail, never the builder, so no layer can observe it — the
        //   harness bridge reads the producer's own answer into
        //   `has_recalled_memory`.
        match (curated_block_present(input), input.has_recalled_memory) {
            (true, true) => output.push_str(
                "`<CuratedMemory>` is already in this prompt, and the auto-recalled \
                 `<memory-context>` message is already in this conversation — don't \
                 search for facts you can already read.\n\
                 \n",
            ),
            (true, false) => output.push_str(
                "`<CuratedMemory>` is already in this prompt — don't search for facts \
                 you can already read.\n\
                 \n",
            ),
            (false, true) => output.push_str(
                "The auto-recalled `<memory-context>` message is already in this \
                 conversation — don't search for facts you can already read.\n\
                 \n",
            ),
            // Neither block landed this turn: no window claim at all. The
            // ladder below still ships — routing is not a runtime fact.
            (false, false) => {}
        }
        output.push_str(
            "Where a NEW memory goes — ONE destination ladder, first matching rung wins:\n\
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
        assert_eq!(layer.priority(), 1745);
        // Dynamic so the priority-zone convention holds (≥1700 = dynamic).
        assert_eq!(layer.stability(), LayerStability::Dynamic);
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

    /// The curated envelope as the harness bridge threads it in, for the tests
    /// that need the window claim to fire.
    fn envelope() -> Option<String> {
        Some("<CuratedMemory>user prefers tabs</CuratedMemory>".to_string())
    }

    /// Render for one of the four (curated, recalled) combinations.
    fn render(curated: bool, recalled: bool) -> String {
        let layer = MemoryProtocolLayer;
        let config = PromptConfig::default();
        let mut out = String::new();
        layer.inject(
            &mut out,
            &LayerInput::basic(&config, &[])
                .with_curated_envelope(curated.then(|| envelope().unwrap()))
                .with_recalled_memory(recalled),
        );
        out
    }

    #[test]
    fn mentions_already_visible_blocks() {
        // Regression — when both blocks ARE in this turn's window, forget to
        // say so and the LLM wastes tool calls re-searching for them. This is a
        // runtime fact about the window, not a tool's semantics, so it is one
        // of the two things that belong in this layer.
        let out = render(true, true);
        assert!(out.contains("CuratedMemory"));
        assert!(out.contains("memory-context"));
        assert!(out.contains("already in this prompt"));
        assert!(out.contains("already in this conversation"));
        assert!(out.contains("don't search for facts you can already read"));
    }

    #[test]
    fn window_claim_names_only_the_block_that_landed() {
        // Two producers, two independent gates. `build_curated_message` does
        // NOT gate on `MemoryInjectionMode`, while `build_memory_user_message`
        // returns `Ok(None)` under `Tools` — so both mixed combinations are
        // reachable in production and each must name exactly what is there.
        let curated_only = render(true, false);
        assert!(curated_only.contains("`<CuratedMemory>` is already in this prompt"));
        assert!(
            !curated_only.contains("memory-context"),
            "must not assert a recall message this turn does not carry: {curated_only}"
        );
        assert!(curated_only.contains("don't search for facts you can already read"));

        let recall_only = render(false, true);
        const RECALL_CLAIM: &str = "`<memory-context>` message is already in this conversation";
        assert!(recall_only.contains(RECALL_CLAIM), "{recall_only}");
        assert!(
            !recall_only.contains("CuratedMemory"),
            "must not assert a curated block this prompt does not carry: {recall_only}"
        );
        assert!(recall_only.contains("don't search for facts you can already read"));
    }

    #[test]
    fn window_claim_is_silent_when_neither_block_landed() {
        // P2 — the claim is runtime fact, not standing advice, and the fact is
        // false in a whole configuration: no curated envelope this turn (empty
        // MEMORY.md and no profile) and, under `MemoryInjectionMode::Tools`, no
        // `<memory-context>` message either. Asserting "already in this
        // conversation" points the model at blocks that are not there, and the
        // only recovery is a wasted turn. Blank counts as absent for the
        // curated half: that is the predicate `CuratedMemoryLayer` itself
        // applies before emitting.
        let layer = MemoryProtocolLayer;
        let config = PromptConfig::default();
        for absent in [None, Some("   ".to_string())] {
            let mut out = String::new();
            layer.inject(
                &mut out,
                &LayerInput::basic(&config, &[])
                    .with_curated_envelope(absent)
                    .with_recalled_memory(false),
            );
            assert!(
                !out.contains("already in this prompt"),
                "must not claim a block it cannot prove: {out}"
            );
            assert!(!out.contains("already in this conversation"), "{out}");
            assert!(!out.contains("memory-context"), "{out}");
            assert!(!out.contains("CuratedMemory"), "{out}");
            assert!(
                !out.contains("don't search for facts you can already read"),
                "no window claim at all when there is no window to claim: {out}"
            );
            // ...and the routing half is untouched by the gate.
            assert!(out.contains("ONE destination ladder"), "{out}");
        }
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
    fn always_injects_regardless_of_input_flags() {
        // P3 contract, NARROWED to the half that is still unconditional: the
        // destination ladder is always-on, not conditional like
        // SessionContextGuideLayer (which only fires after compaction), and no
        // input flag varies it — cross-tool routing applies in every memory
        // configuration. The window claim above it is runtime fact about *this*
        // request and IS conditional (see
        // `window_claim_is_silent_when_neither_block_landed`).
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
        assert_eq!(out_no_session, out_with_session, "ladder must not vary");

        // The window claim only ever PREPENDS — the ladder below it stays
        // byte-identical across all four (curated, recalled) combinations, so
        // gating the claim can never silently reshape the layer's invariant
        // payload.
        const LADDER: &str = "Where a NEW memory goes";
        let tail_of = |out: &str| out[out.find(LADDER).expect("ladder present")..].to_string();
        let expected = tail_of(&out_no_session);
        for (curated, recalled) in [(false, false), (true, false), (false, true), (true, true)] {
            assert_eq!(
                tail_of(&render(curated, recalled)),
                expected,
                "ladder must be byte-identical for (curated={curated}, recalled={recalled})"
            );
        }
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

    #[test]
    fn acknowledgment_contract_is_stated_once_per_writing_tool() {
        // D4 survives the trim, but as one statement per writing tool instead
        // of a fourth copy in the always-on prompt. All three writers must
        // carry it — a tool that drops it silently drops the contract for its
        // own writes, which no amount of prompt prose would restore.
        use crate::tools::AlephTool;
        for desc in [
            <crate::builtin_tools::RememberTool as AlephTool>::DESCRIPTION,
            <crate::builtin_tools::note_manage::NoteManageTool as AlephTool>::DESCRIPTION,
            <crate::builtin_tools::FlagUserCorrectionTool as AlephTool>::DESCRIPTION,
        ] {
            let lower = desc.to_lowercase();
            assert!(
                lower.contains("one short sentence"),
                "missing the one-sentence ack: {desc}"
            );
            assert!(
                lower.contains("user's language"),
                "ack must be in the user's language: {desc}"
            );
            assert!(
                lower.contains("do not quote") || lower.contains("never quote"),
                "ack must not echo stored content: {desc}"
            );
        }
    }
}
