//! The sentences a search answer uses to say what it could not do.
//!
//! # Why these live in one place
//!
//! A dimension the answering backend cannot express, a chain that answered
//! only on its third try, every backend agreeing there is nothing, and a
//! snippet cut to fit a budget all reach the caller as the same vague feeling
//! — "this is less than I asked for" — unless the sentence tells them apart
//! *and* names the lever for each.
//!
//! Two faces owe these sentences: the registry knows about the first three,
//! the tool face about the fourth, and both render into the same `notes` list.
//! `builtin_tools::file_search::notes` exists for the same reason and records
//! what happens without it: written twice the clauses come out near-identical
//! but not identical, and the same clause in two spellings is how a reader
//! learns to skim past it. What legitimately differs between two callers of
//! one clause is a parameter, not a second copy of the sentence.

/// A dimension the answering backend cannot express.
///
/// Names both halves on purpose: without the dimension the reader cannot tell
/// which of their parameters was dropped, and without the backend they cannot
/// tell whether configuring another one would help.
///
/// `dimension` is spelled as the **caller's** parameter (`domains`,
/// `recency`, `full_content`), not as the internal field name — a note that
/// names a lever the reader cannot find is an apology, not a note.
#[must_use]
pub fn degraded(dimension: &str, provider: &str) -> String {
    format!(
        "`{dimension}` was not applied: the answering backend `{provider}` has no native \
         parameter for it, so these results are unfiltered on that axis"
    )
}

/// The chain answered, but not on its first try.
///
/// Worth saying because the results are from a backend the caller did not
/// pick, and because a failure that nobody reports is a failure nobody fixes:
/// the per-backend `name [kind] message` lines are in the log under
/// `target=search`.
#[must_use]
pub fn answered_after_failures(provider: &str, failed: usize) -> String {
    format!(
        "`{provider}` answered after {failed} earlier backend(s) failed; their failures are in \
         the server log under target=search"
    )
}

/// Every backend that was asked came back with zero results.
///
/// Says "answer", not "failure", deliberately: told it failed, a model
/// retries the same query, which is the one thing that cannot help here.
#[must_use]
pub fn all_empty(n: usize) -> String {
    format!(
        "all {n} backend(s) that were asked returned zero results — that is an answer, not a \
         failure; rewording the query will help where retrying it will not"
    )
}

/// Snippets were cut to fit the tool's budget.
///
/// Names the count and the bound so the reader can tell "one long page was
/// trimmed" from "everything you got is a fragment", and names the tool that
/// gets the rest.
#[must_use]
pub fn snippets_clamped(n: usize, max: usize) -> String {
    format!(
        "{n} snippet(s) were clamped to {max} characters; fetch the url with web_fetch for the \
         full page"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every omission has to read as a different thing. When they collapse
    /// into one phrasing ("some results were withheld") readers learn to skip
    /// the line, which costs more than the line saves.
    #[test]
    fn the_four_notes_do_not_read_alike() {
        let all = [
            degraded("domains", "exa"),
            answered_after_failures("brave", 2),
            all_empty(3),
            snippets_clamped(4, 600),
        ];
        for (i, a) in all.iter().enumerate() {
            for b in all.iter().skip(i + 1) {
                assert_ne!(a, b);
            }
        }
    }

    /// A note must name the lever the caller can pull, or it is just an
    /// apology.
    #[test]
    fn the_degraded_note_names_the_dimension_and_the_backend() {
        let n = degraded("domains", "exa");
        assert!(n.contains("domains"), "{n}");
        assert!(n.contains("exa"), "{n}");
    }
}
