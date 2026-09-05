//! Source-level census over the producers of `AUTHOR_USER_KEY`.
//!
//! `run_loop::with_request_scope`'s doc names every origin site for this key.
//! Before 2026-08-28 this census listed only two of them
//! (`handlers::agent::build_run_request`, `inbound_router::executor::execute_for_context_inner`)
//! while a THIRD and FOURTH producer — `teams::broadcast::member_run_metadata`
//! and `teams::dispatcher::runner::task_run_metadata` — had already existed
//! since 2026-08-13 and 2026-08-18 respectively, ten to fifteen days earlier.
//! The census's own author had all four in front of them and named two;
//! grepping the key's name found the doc comment that vouched for two
//! producers, not the other two that were silently uncovered.
//!
//! This census makes that sentence self-enforcing: it first proves the run
//! loop really does seed `CURRENT_ROOM_AUTHOR` from the key, then requires
//! every named origin site to actually write it. Structurally it can only
//! catch a SHRINK of `ORIGIN_SITES` (the floor below), never a new producer
//! elsewhere in the tree that nobody added here — that half is why a fifth
//! producer, `sessions::send_tool::build_sub_metadata`, is unit-tested at its
//! own site (`send_tool.rs::a_background_dispatch_with_no_room_author_writes_no_author_key`
//! and its sibling) rather than folded into `ORIGIN_SITES`: it did not exist
//! as a writer when this census's four were surveyed, so it is not one of
//! the four the doc above is correcting the record about. A future sixth
//! `RunRequest`-metadata builder owes the same two things: a stamp at its own
//! site, and either a new `ORIGIN_SITES` entry here (with the floor raised in
//! the same edit) or a unit test at its own site — not a silent gap.

#[cfg(test)]
mod tests {
    use crate::utils::source_scan::{code_text, production_prefix};

    /// Every file that must stamp `AUTHOR_USER_KEY`, and the function whose
    /// body has to contain the write. Named, not globbed: a producer that
    /// stops stamping must fail by name.
    const ORIGIN_SITES: &[(&str, &str, &str)] = &[
        (
            "src/gateway/handlers/agent.rs",
            include_str!("../../handlers/agent.rs"),
            "build_run_request",
        ),
        (
            "src/gateway/inbound_router/executor.rs",
            include_str!("../../inbound_router/executor.rs"),
            "execute_for_context_inner",
        ),
        (
            "src/teams/broadcast/mod.rs",
            include_str!("../../../teams/broadcast/mod.rs"),
            "member_run_metadata",
        ),
        (
            "src/teams/dispatcher/runner.rs",
            include_str!("../../../teams/dispatcher/runner.rs"),
            "task_run_metadata",
        ),
    ];

    #[test]
    fn the_run_loop_seeds_the_room_author_from_the_author_key() {
        let prod = code_text(&production_prefix(include_str!("mod.rs")));
        assert!(
            prod.contains("AUTHOR_USER_KEY"),
            "run_loop must read AUTHOR_USER_KEY — without this the census below \
             would be requiring producers for a key nobody consumes"
        );
        assert!(
            prod.contains("with_room_author") || prod.contains("CURRENT_ROOM_AUTHOR"),
            "run_loop must seed the room-author task-local from that key"
        );
    }

    #[test]
    fn every_named_origin_site_stamps_the_author_key() {
        let mut checked = 0usize;
        for (path, src, function) in ORIGIN_SITES {
            // Comments and string literals stripped once: `prod.contains(function)`
            // below is the rename-detection canary, and reading it against raw
            // `production_prefix` output would let a stray comment mentioning an
            // old function name satisfy the very check meant to catch a rename —
            // the same defect class this census exists to prevent.
            let prod = code_text(&production_prefix(src));
            assert!(
                prod.contains(function),
                "{path}: the census names `{function}` but that function is not in \
                 the production half of the file — the census input rotted"
            );
            assert!(
                prod.contains("AUTHOR_USER_KEY"),
                "{path}: `run_loop::with_request_scope`'s doc names this file as an \
                 origin site for AUTHOR_USER_KEY, but nothing here stamps it. Either \
                 stamp it, or delete the claim from that doc — a doc comment naming \
                 a producer is not that producer."
            );
            checked += 1;
        }
        // A measured floor alongside the exact count: `checked == ORIGIN_SITES.len()`
        // shrinks with ORIGIN_SITES, so a shortened list would pass it trivially
        // — the one failure this census exists to catch. The floor cannot detect
        // a loop that SKIPS an entry, which is what the exact count is still for;
        // keep both. Raised from 2 to 4 in the same edit that added the two teams
        // producers below — a floor left at the old size would silently sanction
        // shrinking back to them.
        assert!(
            checked >= 4,
            "the census inspected {checked} origin sites; it must cover at least the four \
             producers surveyed when this census was corrected (agent.rs, executor.rs, \
             teams::broadcast, teams::dispatcher::runner) — a shrunken ORIGIN_SITES would \
             otherwise pass `checked == len()` trivially"
        );
        assert_eq!(
            checked,
            ORIGIN_SITES.len(),
            "the census must have inspected every origin site"
        );
    }
}
