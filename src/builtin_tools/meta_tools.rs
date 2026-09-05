//! Fuzzy name matching shared by the tool / slash-command "did you mean" paths.
//!
//! This module used to host the `list_tools` / `search_tools` / `get_tool_schema`
//! meta tools — a two-stage tool-discovery pattern that was never registered in
//! production. Progressive disclosure is served instead by
//! [`crate::tools::tool_search`] and [`crate::tools::schema_lookup`], both
//! registered per-request by the gateway execution engine, so the meta tools
//! were deleted.
//!
//! What survives is the Levenshtein helper they carried, which has real
//! consumers: [`crate::tools::name_repair`] (tool-name repair + the `ToolNotFound`
//! suggestion list) and `tool_metadata::registry::query::suggest_commands`
//! (slash-command "did you mean").

/// Simple Levenshtein distance for fuzzy matching.
///
/// Exposed at `pub(crate)` so the inbound router can reuse it for slash-
/// command "did you mean" suggestions without duplicating the algorithm.
pub(crate) fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    // Prevent OOM on adversarial input
    if a_len > 500 || b_len > 500 {
        return a_len.abs_diff(b_len);
    }

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut matrix = vec![vec![0usize; b_len + 1]; a_len + 1];

    for i in 0..=a_len {
        *matrix
            .get_mut(i)
            .expect("invariant: matrix has a_len + 1 rows")
            .get_mut(0)
            .expect("invariant: each row has b_len + 1 columns") = i;
    }
    for j in 0..=b_len {
        *matrix
            .get_mut(0)
            .expect("invariant: matrix has a_len + 1 rows")
            .get_mut(j)
            .expect("invariant: each row has b_len + 1 columns") = j;
    }

    for i in 1..=a_len {
        for j in 1..=b_len {
            let a_match = a_chars
                .get(i - 1)
                .expect("invariant: i is within a_chars length");
            let b_match = b_chars
                .get(j - 1)
                .expect("invariant: j is within b_chars length");
            let cost = if a_match == b_match { 0 } else { 1 };
            let above = *matrix
                .get(i - 1)
                .expect("invariant: i - 1 is within matrix rows")
                .get(j)
                .expect("invariant: j is within matrix columns")
                + 1;
            let left = *matrix
                .get(i)
                .expect("invariant: i is within matrix rows")
                .get(j - 1)
                .expect("invariant: j - 1 is within matrix columns")
                + 1;
            let diag = *matrix
                .get(i - 1)
                .expect("invariant: i - 1 is within matrix rows")
                .get(j - 1)
                .expect("invariant: j - 1 is within matrix columns")
                + cost;
            *matrix
                .get_mut(i)
                .expect("invariant: i is within matrix rows")
                .get_mut(j)
                .expect("invariant: j is within matrix columns") = above.min(left).min(diag);
        }
    }

    *matrix
        .get(a_len)
        .expect("invariant: a_len is within matrix rows")
        .get(b_len)
        .expect("invariant: b_len is within matrix columns")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_levenshtein_distance() {
        assert_eq!(levenshtein_distance("", ""), 0);
        assert_eq!(levenshtein_distance("abc", ""), 3);
        assert_eq!(levenshtein_distance("", "abc"), 3);
        assert_eq!(levenshtein_distance("abc", "abc"), 0);
        assert_eq!(levenshtein_distance("abc", "abd"), 1);
        assert_eq!(levenshtein_distance("search", "serach"), 2);
        assert_eq!(levenshtein_distance("github", "githu"), 1);
    }
}
