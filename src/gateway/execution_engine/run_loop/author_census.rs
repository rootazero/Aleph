//! Source-level census over the producers of `AUTHOR_USER_KEY`.
//!
//! `run_loop::with_request_scope`'s doc names TWO origin sites for this key.
//! Before 2026-08-28 only one of them existed, and the doc was the only
//! external reference to the missing wire — grepping the key's name found the
//! comment that vouched for the absent producer, not the absence.
//!
//! This census makes that sentence self-enforcing: it first proves the run
//! loop really does seed `CURRENT_ROOM_AUTHOR` from the key, then requires
//! every named origin site to actually write it.

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
        // shrinks with ORIGIN_SITES, so an emptied list would pass it trivially
        // (0 == 0) — the one failure this census exists to catch. The floor
        // cannot detect a loop that SKIPS an entry, which is what the exact
        // count is still for; keep both.
        assert!(
            checked >= 2,
            "the census inspected {checked} origin sites; it must cover at least the two \
             that build a run request — an emptied ORIGIN_SITES would otherwise pass \
             `checked == len()` trivially"
        );
        assert_eq!(
            checked,
            ORIGIN_SITES.len(),
            "the census must have inspected every origin site"
        );
    }
}
