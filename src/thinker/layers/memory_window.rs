//! `MemoryWindowLayer` — "these memory blocks are already in front of you" (priority 1745)
//!
//! Split out of [`MemoryProtocolLayer`] on 2026-08-03. That layer carried two
//! things with opposite cache profiles: a constant destination ladder and this
//! one-sentence claim, which is chosen per turn from
//! `(curated_block_present, has_recalled_memory)`. Rating the pair by its
//! volatile half parked ~1,037 B of never-changing text in the dynamic system
//! block — the half `split_system_blocks_for_cache` leaves without a
//! `cache_control` marker of its own, so it is re-written at 1.25x whenever any
//! volatile neighbour moves (FEATURE_LOCATOR §2.18 ledger item 10).
//!
//! **A layer is classified as one thing; when its halves disagree, the answer is
//! two layers, not the worse of the two ratings.**
//!
//! Sits just before `SessionContextGuideLayer` (1750), the post-compaction
//! guide: both are statements about what this request's window already
//! contains, so they read together.
//!
//! **Bounded by construction, unlike its neighbours in this zone.** The layer
//! renders one of exactly three `&'static str`s and never interpolates session
//! content, so its worst case is a compile-time constant — pinned by
//! `worst_case_render_is_bounded` rather than left to inspection. That is the
//! property `GraphTopologyLayer` did *not* have (it interpolated uncapped
//! human-authored root bodies), and the reason a size claim about a
//! conditionally-silent Dynamic layer has to be asserted somewhere: the
//! `prompt_contract` ratchets measure a production-shaped input under which
//! this layer is silent, so they will never see these bytes.
//!
//! [`MemoryProtocolLayer`]: super::MemoryProtocolLayer

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct MemoryWindowLayer;

/// `<CuratedMemory>` is already in this prompt.
const CURATED_ONLY: &str =
    "`<CuratedMemory>` is already in this prompt — don't search for facts you can \
     already read.\n\n";

/// The auto-recalled `<memory-context>` message is already in this conversation.
const RECALL_ONLY: &str =
    "The auto-recalled `<memory-context>` message is already in this conversation — \
     don't search for facts you can already read.\n\n";

/// Both blocks landed this turn.
const BOTH: &str = "`<CuratedMemory>` is already in this prompt, and the auto-recalled \
     `<memory-context>` message is already in this conversation — don't search for \
     facts you can already read.\n\n";

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

impl PromptLayer for MemoryWindowLayer {
    fn name(&self) -> &'static str {
        "memory_window"
    }

    fn priority(&self) -> u32 {
        1745
    }

    fn stability(&self) -> LayerStability {
        // Genuinely per-turn: `has_recalled_memory` flips whenever
        // `build_memory_user_message` returns `Ok(None)` (empty query, or
        // `MemoryInjectionMode::Tools`), so consecutive turns in one session can
        // render different strings. That is what `Dynamic` is for — and it is
        // the *only* reason this layer is here rather than folded back into the
        // ladder.
        LayerStability::Dynamic
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        !matches!(mode, PromptMode::Minimal)
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        // This is a statement of runtime fact, and it names two blocks that two
        // DIFFERENT producers decide independently — so it is two claims, each
        // gated on its own producer. Merging them (or gating both on one flag)
        // reintroduces the bug: the sentence asserts a block that is not there,
        // the model doesn't search for what it cannot read, and the only
        // recovery is another turn.
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
        let claim = match (curated_block_present(input), input.has_recalled_memory) {
            (true, true) => BOTH,
            (true, false) => CURATED_ONLY,
            (false, true) => RECALL_ONLY,
            // Neither block landed this turn: no claim at all, and no header
            // either — an empty section is still bytes. The destination ladder
            // in `MemoryProtocolLayer` is unaffected; routing is not a runtime
            // fact.
            (false, false) => return,
        };
        output.push_str("\n\n## Memory Window\n");
        output.push_str(claim);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    /// The curated envelope as the harness bridge threads it in, for the tests
    /// that need the window claim to fire.
    fn envelope() -> String {
        "<CuratedMemory>user prefers tabs</CuratedMemory>".to_string()
    }

    /// Render for one of the four (curated, recalled) combinations.
    fn render(curated: bool, recalled: bool) -> String {
        let config = PromptConfig::default();
        let mut out = String::new();
        MemoryWindowLayer.inject(
            &mut out,
            &LayerInput::basic(&config, &[])
                .with_curated_envelope(curated.then(envelope))
                .with_recalled_memory(recalled),
        );
        out
    }

    #[test]
    fn metadata() {
        let layer = MemoryWindowLayer;
        assert_eq!(layer.name(), "memory_window");
        assert_eq!(layer.priority(), 1745);
        assert!(
            layer.priority() >= 1700,
            "per-turn bytes belong in the dynamic suffix"
        );
        assert_eq!(layer.stability(), LayerStability::Dynamic);
        for path in [AssemblyPath::Basic, AssemblyPath::Cached] {
            assert!(layer.paths().contains(&path), "missing path {path:?}");
        }
    }

    #[test]
    fn supports_full_and_compact_not_minimal() {
        let layer = MemoryWindowLayer;
        assert!(layer.supports_mode(PromptMode::Full));
        assert!(layer.supports_mode(PromptMode::Compact));
        assert!(!layer.supports_mode(PromptMode::Minimal));
    }

    #[test]
    fn mentions_already_visible_blocks() {
        // Regression — when both blocks ARE in this turn's window, forget to
        // say so and the LLM wastes tool calls re-searching for them. This is a
        // runtime fact about the window, not a tool's semantics, so it is one
        // of the two things that belong in the memory layers at all.
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
        //
        // Silence must be TOTAL — not even the heading. This is the state
        // `prompt_contract`'s production-shaped input renders, so a stray
        // header here is a byte every install pays forever, in the zone that
        // charges 1.25x for it.
        let config = PromptConfig::default();
        for absent in [None, Some("   ".to_string())] {
            let mut out = String::new();
            MemoryWindowLayer.inject(
                &mut out,
                &LayerInput::basic(&config, &[])
                    .with_curated_envelope(absent)
                    .with_recalled_memory(false),
            );
            assert!(out.is_empty(), "must render nothing at all: {out:?}");
        }
    }

    #[test]
    fn worst_case_render_is_bounded() {
        // The size claim this layer's module docs make, asserted instead of
        // argued. `prompt_contract`'s ratchets measure a production-shaped
        // input under which this layer is silent, so they can never see these
        // bytes — and a conditionally-silent Dynamic layer with no bound of its
        // own is exactly the hole `graph_topology` had (uncapped human-authored
        // root bodies interpolated straight into the 1.25x zone).
        //
        // Bounded here by construction: the layer picks one of three consts and
        // never interpolates session content. Raising this number means a claim
        // grew; interpolating anything means the bound is gone entirely.
        const CEILING_BYTES: usize = 320;
        for (curated, recalled) in [(false, false), (true, false), (false, true), (true, true)] {
            let out = render(curated, recalled);
            assert!(
                out.len() <= CEILING_BYTES,
                "(curated={curated}, recalled={recalled}) renders {} B, ceiling {CEILING_BYTES}",
                out.len()
            );
        }

        // ...and the bound holds regardless of how big the curated envelope is,
        // which is the property that makes it a bound at all.
        let config = PromptConfig::default();
        let mut out = String::new();
        MemoryWindowLayer.inject(
            &mut out,
            &LayerInput::basic(&config, &[])
                .with_curated_envelope(Some("x".repeat(100_000)))
                .with_recalled_memory(true),
        );
        assert!(
            out.len() <= CEILING_BYTES,
            "session content reached the render — the bound is gone: {} B",
            out.len()
        );
    }
}
