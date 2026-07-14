//! Fuzzy name matching shared by the tool / slash-command "did you mean" paths.
//!
//! This module used to host the `list_tools` / `search_tools` / `get_tool_schema`
//! meta tools — a two-stage tool-discovery pattern that was only ever registered
//! when `BuiltinToolConfig.tool_catalog` was `Some`, which production never set.
//! Progressive disclosure is served instead by [`crate::tools::tool_search`] and
//! [`crate::tools::schema_lookup`], both registered per-request by the gateway
//! execution engine, so the meta tools were deleted.
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

    for (i, row) in matrix.iter_mut().enumerate().take(a_len + 1) {
        row[0] = i;
    }
    for (j, val) in matrix[0].iter_mut().enumerate().take(b_len + 1) {
        *val = j;
    }

    for i in 1..=a_len {
        for j in 1..=b_len {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            matrix[i][j] = (matrix[i - 1][j] + 1)
                .min(matrix[i][j - 1] + 1)
                .min(matrix[i - 1][j - 1] + cost);
        }
    }

    matrix[a_len][b_len]
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
