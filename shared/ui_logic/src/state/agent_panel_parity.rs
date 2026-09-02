//! Parity property test for the agent panel's ordering (Task 10, R10-3).
//!
//! `#[cfg(test)]`-only: declared as `#[cfg(test)] mod agent_panel_parity;`
//! in `state/mod.rs`, with no `pub use` (R10-7) — re-exporting a test-only
//! module is either a compile error under `cargo build` or a `pub` surface
//! that only exists in test builds, and this crate wants neither.
//!
//! # What this proves, and what it does not
//!
//! R2 says sorting lives ONLY in [`super::agent_panel::sort_entries`]. Both
//! frontends (`interfaces/tui` and `interfaces/webchat`) call it on a clone
//! and never sort on their own — so "the TUI and the Panel render the same
//! order" reduces to "`sort_entries` is a deterministic, total function of
//! its input", which is exactly what `sorting_is_deterministic_and_total`
//! checks below.
//!
//! What it does NOT check is whether either frontend actually calls
//! `sort_entries` rather than rolling its own `.sort_by`/`.sort()`. This
//! crate has no dependency on either frontend crate (and no dependency on
//! `alephcore`, so it cannot source-scan them either — R10-7 confirms
//! `shared/ui_logic/Cargo.toml` has no `alephcore` dep). That half of the
//! guarantee is a separate, source-level guard in `alephcore`
//! (`src/gateway/runtime/mod.rs`, added by this same task) that scans both
//! frontend files for a self-rolled ordering call. The two tests together
//! are what make "both faces show one order" enforced instead of merely
//! documented; neither one alone does.

use aleph_protocol::runtime::{RuntimeAgentEntry, RuntimeAgentState};

use super::agent_panel::{attention_rank, sort_entries};

/// Four [`RuntimeAgentEntry`]s, in every possible input order (4! = 24
/// permutations) — carrying ONE deliberate tie (Task 10 fix round 1, F3):
/// two entries share both `state` (`Idle`) and `updated_at` (0, same as
/// the other two), so `attention_rank` alone cannot decide their relative
/// order and the `session_id` tie-break key must fire for the result to be
/// a total order at all.
///
/// Written with a tiny recursive helper (Heap's algorithm) rather than by
/// hand or by pulling in `permutohedron`/`itertools` for 24 cases (R10-3).
/// Each permutation carries a FRESH clone of the four entries — they are not
/// `Copy` (they own `String` fields) — so mutating one permutation's vector
/// in the test below can never alias another's.
fn all_permutations_of_four_entries() -> Vec<Vec<RuntimeAgentEntry>> {
    fn entry(session_id: &str, state: RuntimeAgentState) -> RuntimeAgentEntry {
        RuntimeAgentEntry {
            session_id: session_id.to_string(),
            label: "claude".to_string(),
            cwd: String::new(),
            agent: None,
            state,
            updated_at: 0,
        }
    }

    // "second" and "first" tie on state AND updated_at, and are listed here
    // in the OPPOSITE of `session_id` order — so if the `session_id`
    // tie-break key is ever dropped, reordered ahead of `state`, or
    // reversed, at least one of the 24 permutations below sorts them the
    // wrong way relative to the others and `sorting_is_deterministic_and_total`
    // reddens. Single-key coverage (`Blocked` outranks `Working`) is
    // exercised separately by `agent_panel`'s own unit tests
    // (`blocked_always_outranks_working`), not here.
    let base = [
        entry("blocked", RuntimeAgentState::Blocked),
        entry("working", RuntimeAgentState::Working),
        entry("second", RuntimeAgentState::Idle),
        entry("first", RuntimeAgentState::Idle),
    ];

    // Heap's algorithm over indices 0..4, generating all 24 permutations by
    // index so the entries themselves are cloned (not moved) into each
    // output permutation.
    fn heap(k: usize, indices: &mut [usize; 4], out: &mut Vec<[usize; 4]>) {
        if k == 1 {
            out.push(*indices);
            return;
        }
        for i in 0..k {
            heap(k - 1, indices, out);
            if k % 2 == 0 {
                indices.swap(i, k - 1);
            } else {
                indices.swap(0, k - 1);
            }
        }
    }

    let mut index_perms = Vec::new();
    let mut indices = [0usize, 1, 2, 3];
    heap(4, &mut indices, &mut index_perms);

    index_perms
        .into_iter()
        .map(|perm| perm.into_iter().map(|i| base[i].clone()).collect())
        .collect()
}

/// R2 的自动化表达：两端都只能通过 sort_entries 得到顺序，
/// 所以「TUI 与 Panel 显示同一个顺序」等价于「两边都调了它」。
/// 这条守卫钉住的是后者——任何一端自己排序，property 就会漂。
///
/// Task 10 fix round 1, F3: the previous version cloned ONE permutation
/// into `a` and `b` and sorted both — two runs of a pure function on
/// identical input are equal for any implementation, so that compared
/// nothing (判据 §2, unfalsifiable by construction) and totality went
/// untested (dropping the `session_id` tie-break key still left it green).
/// This version instead sorts every one of the 24 permutations and compares
/// each result against the FIRST permutation's sorted result: a real
/// parity property — "every input order of the same set sorts to the same
/// output" — that reddens on a dropped, reordered, or reversed
/// `session_id` key thanks to the tie built into the fixture above.
#[test]
fn sorting_is_deterministic_and_total() {
    let mut permutations = all_permutations_of_four_entries().into_iter();
    let mut canonical = permutations.next().expect("generator always yields 24 permutations");
    sort_entries(&mut canonical);
    assert!(canonical
        .windows(2)
        .all(|w| attention_rank(w[0].state) <= attention_rank(w[1].state)));

    for perm in permutations {
        let mut sorted = perm;
        sort_entries(&mut sorted);
        assert_eq!(
            sorted, canonical,
            "sort must not depend on input order — every permutation of the \
             same entries must sort to the same output"
        );
    }
}

/// `all_permutations_of_four_entries` claims 24 permutations of four
/// entries; a bug in `heap` (e.g. an off-by-one that produces duplicates or
/// drops a permutation) would silently shrink the coverage of the test
/// above without it ever going red — this pins the generator's own claim.
///
/// Task 10 fix round 1, F4: dedups on `session_id`, not `state`. The
/// fixture now carries two entries that share a `state` (the tie F3
/// needs), so projecting onto `state` collapses every permutation that
/// only swaps those two `Idle` entries with each other — undercounting to
/// 12 distinct orderings instead of 24, which would have silently
/// re-broken F3's fix (it needs all 24 genuinely distinct orderings on
/// hand). `session_id` is unique per entry by construction here, so this
/// projection is injective without depending on the fixture's state shape.
#[test]
fn the_generator_produces_all_twenty_four_permutations_with_no_duplicates() {
    let perms = all_permutations_of_four_entries();
    assert_eq!(perms.len(), 24);

    let mut orderings: Vec<Vec<&str>> = perms
        .iter()
        .map(|perm| perm.iter().map(|e| e.session_id.as_str()).collect())
        .collect();
    orderings.sort();
    orderings.dedup();
    assert_eq!(orderings.len(), 24, "generator produced a duplicate ordering");
}
