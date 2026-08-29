//! The clauses `grep` and `find` append to their `message`.
//!
//! # Why these live in one place
//!
//! An ignored directory, a denied credential path, a hit walk cap and an
//! exhausted page all read identically to a caller — "fewer results than I
//! expected" — unless the message tells them apart *and* names the lever for
//! each. Both tools owe every one of those sentences, and when each wrote its
//! own they came out near-identical but not identical: the same clause with
//! two spellings is how a caller learns to skim past it.
//!
//! So the wording is here, once, and the two tools compose the clauses they
//! have facts for. What legitimately differs between two callers of the same
//! clause is a parameter, not a second copy of the sentence.

use super::walk::{WalkReport, MAX_WALK_FILES};

/// "There is more; here is the exact call that gets it."
///
/// Names the cursor *and* the wider page, because the two answer different
/// questions: `offset` continues, `limit` avoids the round trip next time.
pub(super) fn paging(next_offset: usize, limit: usize) -> String {
    format!(
        ". More available — pass offset={next_offset} for the next page, or limit={} for a \
         bigger one",
        limit.saturating_mul(2)
    )
}

/// "Something was excluded because the repository ignores it."
///
/// Emitted whenever ignore rules were in force — not only when
/// [`WalkReport::floor_skipped_dirs`] is non-zero. That counter sees only the
/// directories *this* module's own filter refused; `ignore` drops gitignored
/// trees before the filter runs, so a zero there is not evidence that nothing
/// was excluded, and a message conditioned on it would stay silent in exactly
/// the case the caller most needs told.
///
/// The same argument binds the **call site**, and that is where it was first
/// broken: both tools appended this only on the "found nothing" branch, so a
/// caller who got sixty matches was never told there might be more in the
/// trees the repository ignores — which is the case they are about to act on.
/// It belongs beside [`withheld`], after whatever the result did report, since
/// the two answer the same question about the same walk.
pub(super) fn ignored(report: &WalkReport, respected_ignore: bool) -> Option<String> {
    if !respected_ignore {
        return None;
    }
    let mut note = String::from(
        ". Ignored and generated files were excluded; pass no_ignore=true to search them",
    );
    if report.floor_skipped_dirs > 0 {
        note.push_str(&format!(
            " ({} generated/VCS dir(s) skipped by name)",
            report.floor_skipped_dirs
        ));
    }
    Some(note)
}

/// "A protected location was in the way." Never silent: a credential directory
/// dropping out of a result set looks exactly like it not existing.
pub(super) fn withheld(denied: usize) -> Option<String> {
    (denied > 0).then(|| format!(". {denied} path(s) withheld by the protected-location floor"))
}

/// "The tree is bigger than one call can walk."
///
/// Names `path` and only `path`. The obvious other lever is the glob, and it
/// is the wrong advice: the glob filters the walk's *output*, so narrowing it
/// leaves the traversal exactly as long and the caller retries forever against
/// an unchanged bound. `no_ignore` is worth naming in the other direction —
/// when it is on, the generated trees are usually most of what was walked.
pub(super) fn walk_capped(report: &WalkReport, respected_ignore: bool) -> Option<String> {
    report.walk_capped.then(|| {
        let widened = if respected_ignore {
            ""
        } else {
            "; no_ignore=true is walking the ignored and generated trees too, which are \
             usually the bulk of them"
        };
        format!(
            ". Walk stopped after visiting {MAX_WALK_FILES} files — narrow `path`{widened}. \
             A narrower glob does not help: it filters the result, not the walk"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(floor_skipped_dirs: usize, walk_capped: bool) -> WalkReport {
        WalkReport {
            files: Vec::new(),
            walk_capped,
            denied: 0,
            floor_skipped_dirs,
        }
    }

    #[test]
    fn paging_names_both_levers() {
        let note = paging(60, 60);
        assert!(note.contains("offset=60"), "{note}");
        assert!(note.contains("limit=120"), "{note}");
    }

    /// The lever is named whenever it could have changed the answer, which is
    /// whenever ignore rules were on — not only when the floor counter fired.
    #[test]
    fn the_ignore_lever_is_named_even_with_a_zero_floor_count() {
        let note = ignored(&report(0, false), true).expect("ignore was in force");
        assert!(note.contains("no_ignore=true"), "{note}");
        assert!(!note.contains("skipped by name"), "{note}");

        let counted = ignored(&report(3, false), true).unwrap();
        assert!(counted.contains("3 generated/VCS dir(s)"), "{counted}");

        assert!(ignored(&report(0, false), false).is_none());
    }

    #[test]
    fn withheld_is_silent_only_when_nothing_was_withheld() {
        assert!(withheld(0).is_none());
        assert!(withheld(2).unwrap().contains("2 path(s) withheld"));
    }

    /// The lever named is `path`, and the one deliberately NOT named is the
    /// glob — narrowing it cannot lift this bound, so advising it would send
    /// the caller round a loop that never terminates.
    #[test]
    fn the_walk_cap_names_the_only_lever_that_can_lift_it() {
        assert!(walk_capped(&report(0, false), true).is_none());

        let note = walk_capped(&report(0, true), true).unwrap();
        assert!(note.contains("`path`"), "{note}");
        assert!(!note.contains("no_ignore"), "{note}");

        let widened = walk_capped(&report(0, true), false).unwrap();
        assert!(widened.contains("no_ignore=true"), "{widened}");
    }
}
