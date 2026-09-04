//! What makes two results from two backends "the same result".
//!
//! # Why this module exists now and not before
//!
//! Until a call could ask more than one backend, every result in an answer
//! came from one place and `SearchResult::provider` had no consumer — the
//! previous round wrote that down as the condition under which it would gain
//! one. Consulting several backends is that condition: the same page comes
//! back from two of them under two spellings of one url, and without a notion
//! of identity the caller pays context for the duplicate and cannot tell it
//! *is* one.
//!
//! # The one rule that keeps a wrong guess cheap
//!
//! [`identity`] is used **only** to decide whether two results are the same
//! one. The url that reaches the caller is always the one its backend sent,
//! untouched. So a normalisation rule that is too aggressive costs a merged
//! pair that should have stayed apart, and one that is too timid costs a
//! duplicate — neither can hand anyone a url that does not resolve. That is
//! deliberate: the rules below are heuristics about how vendors decorate
//! links, and heuristics age (D.0.5 — a list only covers the world on the day
//! it was written).

use crate::search::SearchResult;
use std::collections::HashSet;

/// Query parameters that identify a *referrer*, not a document.
///
/// `utm_` is matched as a prefix because it is a whole family (`utm_source`,
/// `utm_medium`, …) and enumerating it would be the kind of list that is
/// wrong the first time a vendor adds a member. The rest are the handful of
/// single-name trackers common enough that two search backends routinely
/// disagree about them for the same page.
const TRACKING_PREFIXES: &[&str] = &["utm_"];
const TRACKING_PARAMS: &[&str] = &["gclid", "fbclid", "ref", "ref_src", "mc_cid", "mc_eid"];

fn is_tracking(key: &str) -> bool {
    TRACKING_PREFIXES.iter().any(|p| key.starts_with(p))
        || TRACKING_PARAMS.iter().any(|p| key.eq_ignore_ascii_case(p))
}

/// The key two backends would agree on for the same page.
///
/// Lower-cases scheme and host, drops a leading `www.`, drops the default
/// port, drops the fragment, drops tracking parameters, sorts the parameters
/// that remain (two backends can order them differently for one link), and
/// drops a trailing slash from the path.
///
/// A url that does not parse is its own identity, trimmed and lower-cased —
/// never dropped. "I could not normalise this" must not become "this is the
/// same as that one", and it must not become "discard it" either.
#[must_use]
pub(crate) fn identity(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url.trim()) else {
        return url.trim().to_lowercase();
    };

    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(k, _)| !is_tracking(k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let mut kept = kept;
    kept.sort();

    parsed.set_fragment(None);
    if kept.is_empty() {
        parsed.set_query(None);
    } else {
        let mut serializer = parsed.query_pairs_mut();
        serializer.clear();
        for (k, v) in &kept {
            serializer.append_pair(k, v);
        }
        drop(serializer);
    }
    // `set_port(None)` restores the scheme's default, which is exactly the
    // normalisation wanted; it only fails for schemes that cannot have a
    // port (`mailto:`), where there is nothing to strip anyway.
    if parsed.port() == parsed.port_or_known_default() {
        let _ = parsed.set_port(None);
    }

    let scheme = parsed.scheme().to_lowercase();
    let host = parsed
        .host_str()
        .unwrap_or_default()
        .to_lowercase()
        .trim_start_matches("www.")
        .to_string();
    let port = parsed.port().map_or_else(String::new, |p| format!(":{p}"));
    let path = parsed.path().trim_end_matches('/');
    let query = parsed.query().map_or_else(String::new, |q| format!("?{q}"));
    format!("{scheme}://{host}{port}{path}{query}")
}

/// Interleave several backends' answers by rank, drop repeats, bound the
/// total.
///
/// By rank rather than by concatenation: a caller who asked two backends
/// wants both opinions, and appending one list to the other would bury the
/// second backend's best result behind the first backend's worst. Round `n`
/// takes every backend's `n`-th result, in the order the caller named them,
/// so the head of the merged list is "what each backend thought was best".
///
/// Returns the merged results and how many repeats were dropped — the count
/// is what lets the answer explain why a request for ten came back with
/// seven.
pub(crate) fn merge_by_rank(
    per_backend: Vec<Vec<SearchResult>>,
    max_results: usize,
) -> (Vec<SearchResult>, usize) {
    let (indexed, duplicates) = merge_by_rank_indexed(per_backend, max_results);
    (
        indexed.into_iter().map(|(_, result)| result).collect(),
        duplicates,
    )
}

/// [`merge_by_rank`], keeping each merged result's source index.
///
/// The one merge implementation, indexed: multi-query search needs to know
/// *which query* a surviving row came from, and re-deriving that afterwards
/// from the url would be a second, drift-prone statement of the identity
/// rule above. [`merge_by_rank`] is the same merge with the index dropped,
/// so the two can never disagree about what merged where.
///
/// Empty source lists are legal and keep the indices of the remaining
/// sources aligned with the caller's own numbering — a query that failed or
/// found nothing contributes an empty list rather than shifting everyone
/// else's attribution.
pub(crate) fn merge_by_rank_indexed(
    per_source: Vec<Vec<SearchResult>>,
    max_results: usize,
) -> (Vec<(usize, SearchResult)>, usize) {
    let deepest = per_source.iter().map(Vec::len).max().unwrap_or(0);
    let mut seen: HashSet<String> = HashSet::new();
    let mut merged: Vec<(usize, SearchResult)> = Vec::new();
    let mut duplicates = 0usize;

    for rank in 0..deepest {
        for (source, results) in per_source.iter().enumerate() {
            let Some(result) = results.get(rank) else {
                continue;
            };
            if seen.insert(identity(&result.url)) {
                merged.push((source, result.clone()));
            } else {
                duplicates += 1;
            }
        }
    }
    // Truncate after deduplication, not before: cutting first would spend the
    // budget on repeats and then report fewer results than the caller could
    // have had.
    merged.truncate(max_results);
    (merged, duplicates)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(url: &str, provider: &str) -> SearchResult {
        let mut result = SearchResult::new("t", url, "s");
        result.provider = Some(provider.to_string());
        result
    }

    #[test]
    fn the_decorations_two_backends_disagree_about_do_not_make_two_results() {
        let canonical = identity("https://example.com/a");
        for spelling in [
            "https://www.example.com/a",
            "https://EXAMPLE.com/a",
            "https://example.com:443/a",
            "https://example.com/a/",
            "https://example.com/a#section",
            "https://example.com/a?utm_source=x&utm_medium=y",
            "https://example.com/a?gclid=123",
            "  https://example.com/a  ",
        ] {
            assert_eq!(identity(spelling), canonical, "{spelling}");
        }
    }

    #[test]
    fn a_parameter_that_selects_content_is_not_a_decoration() {
        assert_ne!(
            identity("https://example.com/watch?v=abc"),
            identity("https://example.com/watch")
        );
        // Order is not identity: one backend may re-serialize the query.
        assert_eq!(
            identity("https://example.com/p?b=2&a=1"),
            identity("https://example.com/p?a=1&b=2")
        );
        // http and https are different origins and stay different results.
        assert_ne!(
            identity("http://example.com/a"),
            identity("https://example.com/a")
        );
    }

    /// A url that will not parse keeps its own identity rather than
    /// collapsing with every other unparseable one — `""` as a shared key
    /// would merge unrelated results into a single row.
    #[test]
    fn an_unparseable_url_is_its_own_identity() {
        assert_ne!(identity("not a url"), identity("also not a url"));
        assert_eq!(identity("Not A Url"), identity("not a url"));
    }

    #[test]
    fn merging_interleaves_by_rank_so_neither_backend_is_buried() {
        let a = vec![r("https://a1.test", "a"), r("https://a2.test", "a")];
        let b = vec![r("https://b1.test", "b"), r("https://b2.test", "b")];
        let (merged, dupes) = merge_by_rank(vec![a, b], 10);
        assert_eq!(dupes, 0);
        let urls: Vec<&str> = merged.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            urls,
            vec![
                "https://a1.test",
                "https://b1.test",
                "https://a2.test",
                "https://b2.test"
            ]
        );
    }

    #[test]
    fn a_page_both_backends_found_appears_once_and_is_counted() {
        let a = vec![r("https://shared.test/x", "a")];
        let b = vec![r("https://www.shared.test/x?utm_source=b", "b")];
        let (merged, dupes) = merge_by_rank(vec![a, b], 10);
        assert_eq!(merged.len(), 1);
        assert_eq!(dupes, 1);
        assert_eq!(
            merged[0].provider.as_deref(),
            Some("a"),
            "the first backend to report it keeps the attribution"
        );
        assert_eq!(
            merged[0].url, "https://shared.test/x",
            "the url handed back is the one its backend sent, not the identity"
        );
    }

    /// Cutting to `max_results` before deduplication would spend the budget
    /// on repeats: two backends that agree on the top three would fill a
    /// limit of three with one distinct page.
    #[test]
    fn the_bound_is_applied_to_distinct_results_not_to_repeats() {
        let a = vec![
            r("https://p1.test", "a"),
            r("https://p2.test", "a"),
            r("https://p3.test", "a"),
        ];
        let b = vec![
            r("https://p1.test", "b"),
            r("https://p2.test", "b"),
            r("https://q1.test", "b"),
        ];
        let (merged, dupes) = merge_by_rank(vec![a, b], 3);
        assert_eq!(dupes, 2);
        let urls: Vec<&str> = merged.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            urls,
            vec!["https://p1.test", "https://p2.test", "https://p3.test"]
        );
    }

    #[test]
    fn a_backend_that_answered_nothing_does_not_shift_the_others() {
        let a = vec![r("https://a1.test", "a")];
        let (merged, dupes) = merge_by_rank(vec![vec![], a, vec![]], 10);
        assert_eq!(dupes, 0);
        assert_eq!(merged.len(), 1);
    }

    /// The indexed variant is the same merge with provenance attached: a
    /// surviving row names the list it came from, an empty list consumes its
    /// index without shifting anyone else's, and a repeat is attributed to
    /// the first list that reported it.
    #[test]
    fn the_indexed_merge_keeps_each_rows_source() {
        let a = vec![r("https://shared.test/x", "a"), r("https://a1.test", "a")];
        let b: Vec<SearchResult> = vec![];
        let c = vec![r("https://www.shared.test/x?utm_source=c", "c"), r("https://c1.test", "c")];
        let (merged, dupes) = merge_by_rank_indexed(vec![a, b, c], 10);
        assert_eq!(dupes, 1, "the decorated repeat of the shared page");
        let rows: Vec<(usize, &str)> = merged
            .iter()
            .map(|(source, r)| (*source, r.url.as_str()))
            .collect();
        assert_eq!(
            rows,
            vec![
                (0, "https://shared.test/x"),
                (0, "https://a1.test"),
                (2, "https://c1.test"),
            ],
            "rank-interleaved, empty list skipped, repeat kept by first reporter"
        );
        assert_eq!(
            merged[0].1.provider.as_deref(),
            Some("a"),
            "the surviving row is the first source's own copy"
        );
    }
}
