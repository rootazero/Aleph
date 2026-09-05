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

/// One backend's failure, phrased so the reader knows which lever fixes it.
///
/// The line every failure report is built from — the chain's "All search
/// providers failed" and the fan-out's "All named search providers failed"
/// are lists of these. The `name [kind] message` framing is load-bearing
/// (the log's `kind=` token and the report's `[kind]` are the same
/// vocabulary, so a line the model reads and a line an operator greps agree);
/// what the kind adds on top is the *guidance clause*: which of the two
/// moves — go fix something, or wait — this failure calls for. Without it,
/// an expired key and an exhausted quota read identically, and the reader
/// waits for the first while the second never heals.
///
/// The clause is a function of the kind alone: the same failure mode gets
/// the same sentence whoever produced it, which is what keeps the report
/// readable when three backends fail three different ways.
#[must_use]
pub fn failure_line(
    provider: &str,
    kind: crate::search::error::SearchErrorKind,
    error: &crate::error::AlephError,
) -> String {
    use crate::search::error::SearchErrorKind as K;
    let guidance = match kind {
        K::Auth => "its credential was rejected — check this backend's API key; this does not heal on its own",
        K::Config => "its configuration is the problem — fix this backend's settings; this does not heal on its own",
        K::Quota => "its quota or rate limit is exhausted — retry later, or raise the plan",
        K::Transient => "a transient failure — retrying later may help",
        K::InvalidResponse => "its response did not parse — the backend may have changed, or answered a challenge page",
        K::RequestRejected => "it rejected the request as shaped — check the options being sent to it",
        K::Cancelled => "the caller cancelled the search",
        // No honest lever to name: the error text is all there is.
        K::Other => "",
    };
    let label = kind.as_str();
    if guidance.is_empty() {
        format!("{provider} [{label}] {error}")
    } else {
        format!("{provider} [{label}] {error} — {guidance}")
    }
}

/// The chain answered, but not on its first try.
///
/// Worth saying because the results are from a backend the caller did not
/// pick, and because a failure that nobody reports is a failure nobody fixes:
/// the per-backend `name [kind] message` lines are in the log under
/// `target=search`.
///
/// Counts only backends that were actually *asked* and errored. A backend
/// the chain skipped because it reports itself unavailable never received a
/// request, so counting it here would report a failure that did not happen —
/// it gets its own sentence ([`answered_past_unavailable`]).
#[must_use]
pub fn answered_after_failures(provider: &str, failed: usize) -> String {
    format!(
        "`{provider}` answered after {failed} earlier backend(s) failed; their failures are in \
         the server log under target=search"
    )
}

/// The chain answered, but some configured backends were never asked at all:
/// they report themselves unavailable (missing configuration), so they cost
/// no request and produce no failure.
///
/// Distinct from [`answered_after_failures`] because the lever differs: a
/// failure may heal on its own (a rate limit resets), an unavailable backend
/// stays skipped until the operator fixes its configuration — and the note
/// has to say *skipped*, because "failed" would send the reader hunting the
/// log for a request that was never made.
#[must_use]
pub fn answered_past_unavailable(skipped: usize) -> String {
    format!(
        "{skipped} configured backend(s) were skipped as unavailable (missing configuration) \
         without being asked; their names are in the server log under target=search"
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

/// Some of the named backends did not answer, and the rest did.
///
/// Distinct from [`answered_after_failures`] on purpose: that one is about a
/// chain, where an earlier backend failing is invisible bookkeeping because a
/// later one produced the whole answer. Here every named backend was supposed
/// to contribute, so a missing one means the answer is narrower than the
/// caller asked for — and the caller, having named the backends, is the one
/// person who can act on which of them is down.
#[must_use]
pub fn fanout_partial(answered: usize, asked: usize) -> String {
    format!(
        "{answered} of {asked} named backend(s) answered; the rest failed and their errors are \
         in the server log under target=search"
    )
}

/// The same page came back from more than one backend.
///
/// Says how many, because it is the difference between "you asked for ten and
/// got seven because the backends agree a lot" and "you asked for ten and
/// there are only seven pages". Names the field that survives the merge so
/// the reader can see which backend each kept result came from.
#[must_use]
pub fn merged_duplicates(n: usize) -> String {
    format!(
        "{n} result(s) were returned by more than one backend and merged; each result's \
         `provider` names the backend it came from"
    )
}

/// Prefix a per-query fact with the query it is true of.
///
/// A multi-query answer aggregates several queries' notes into one list, and
/// a fact like "every backend returned zero results" is true of *one* of
/// them — unattributed it would read as a statement about the whole call,
/// which is exactly the misreading the note exists to prevent.
#[must_use]
pub fn for_query(query: &str, note: &str) -> String {
    format!("query `{query}`: {note}")
}

/// One query of a multi-query call failed outright; the rest still answered.
///
/// Mirrors [`fanout_partial`]'s rule: the caller named the queries, so the
/// caller is the one who can act on one of them being down — and the
/// per-backend failure report is already in the server log, so the note
/// points there rather than repeating it.
#[must_use]
pub fn query_failed(query: &str) -> String {
    format!(
        "query `{query}` failed on every backend that tried it, so the merged set has no \
         results for it; the failure report is in the server log under target=search"
    )
}

/// The same page came back from more than one *query*.
///
/// Distinct from [`merged_duplicates`], whose attribution claim is false
/// here: the duplicates were found by different queries, possibly on the
/// same backend, and the surviving row keeps the first query that returned
/// it, not a provider name.
#[must_use]
pub fn merged_duplicates_across_queries(n: usize) -> String {
    format!(
        "{n} result(s) were found by more than one query and merged; each result's \
         `query_index` names the first query that returned it"
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

/// Page bodies were cut to fit the tool's budget.
///
/// Kept distinct from [`snippets_clamped`] because the levers differ: a
/// clamped snippet is completed by fetching the url, a truncated body is
/// already the fetch, and the reader's next move is to narrow the query
/// instead.
#[must_use]
pub fn full_content_truncated(n: usize, max: usize) -> String {
    format!(
        "{n} page body(s) were truncated at {max} characters; ask for fewer results, or search \
         for the part of the page you need"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every omission has to read as a different thing. When they collapse
    /// into one phrasing ("some results were withheld") readers learn to skip
    /// the line, which costs more than the line saves.
    #[test]
    fn no_two_notes_read_alike() {
        let all = [
            degraded("domains", "exa"),
            answered_after_failures("brave", 2),
            answered_past_unavailable(1),
            all_empty(3),
            snippets_clamped(4, 600),
            full_content_truncated(2, 20_000),
            fanout_partial(2, 3),
            merged_duplicates(4),
            for_query("q", &all_empty(2)),
            query_failed("q"),
            merged_duplicates_across_queries(3),
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

    /// The failure line's whole reason to exist: two failure modes that need
    /// opposite reader behaviour (fix the key vs wait out the window) must
    /// not read alike.
    #[test]
    fn failure_lines_tell_fixing_from_waiting() {
        use crate::error::AlephError;
        use crate::search::error::SearchErrorKind as K;
        let auth = failure_line(
            "tavily",
            K::Auth,
            &AlephError::authentication("tavily", "401"),
        );
        let quota = failure_line("brave", K::Quota, &AlephError::rate_limit("429"));
        assert!(auth.contains("tavily [auth]"), "{auth}");
        assert!(auth.contains("API key"), "{auth}");
        assert!(quota.contains("brave [quota]"), "{quota}");
        assert!(quota.contains("retry later"), "{quota}");
        assert_ne!(auth, quota);
    }

    /// Every kind produces the `name [kind]` framing the log and the report
    /// share, and `Other` — the one kind with no honest lever — carries no
    /// made-up guidance.
    #[test]
    fn every_failure_line_keeps_the_kind_framing() {
        use crate::error::AlephError;
        use crate::search::error::SearchErrorKind as K;
        let err = AlephError::network("boom");
        for kind in [
            K::Transient,
            K::Quota,
            K::Auth,
            K::Config,
            K::InvalidResponse,
            K::RequestRejected,
            K::Cancelled,
            K::Other,
        ] {
            let line = failure_line("p", kind, &err);
            assert!(
                line.starts_with(&format!("p [{}]", kind.as_str())),
                "{line}",
            );
        }
        let other = failure_line("p", K::Other, &err);
        assert!(!other.contains('—'), "no invented lever for Other: {other}");
    }
}
