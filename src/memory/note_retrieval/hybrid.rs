//! Reciprocal Rank Fusion (RRF) helper for combining ranked result lists.

/// Reciprocal Rank Fusion — combines multiple ranked lists into one.
///
/// For each item in each list, accumulates score `1 / (k + rank + 1)`.
/// Items that appear in multiple lists accumulate higher scores.
/// The constant k=60 is the standard RRF default.
pub fn rrf_fuse<T: Clone + Eq + std::hash::Hash>(
    lists: Vec<Vec<T>>,
    k: f32,
    limit: usize,
) -> Vec<(T, f32)> {
    use std::collections::HashMap;

    let mut scores: HashMap<T, f32> = HashMap::new();
    for list in lists {
        for (rank, item) in list.into_iter().enumerate() {
            let rrf = 1.0 / (k + (rank as f32) + 1.0);
            *scores.entry(item).or_insert(0.0) += rrf;
        }
    }

    let mut sorted: Vec<(T, f32)> = scores.into_iter().collect();
    sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    sorted.truncate(limit);
    sorted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_with_single_list_preserves_order() {
        let list = vec!["a", "b", "c"];
        let fused = rrf_fuse(vec![list], 60.0, 10);
        assert_eq!(fused[0].0, "a");
        assert_eq!(fused[1].0, "b");
        assert_eq!(fused[2].0, "c");
    }

    #[test]
    fn rrf_prefers_items_in_multiple_lists() {
        let list_a = vec!["a", "b", "c"];
        let list_b = vec!["b", "d", "e"];
        let fused = rrf_fuse(vec![list_a, list_b], 60.0, 10);
        // "b" appears in both — should have highest fused score
        assert_eq!(fused[0].0, "b");
    }

    #[test]
    fn rrf_limit_truncates() {
        let list = vec!["a", "b", "c", "d", "e"];
        let fused = rrf_fuse(vec![list], 60.0, 2);
        assert_eq!(fused.len(), 2);
    }

    #[test]
    fn rrf_empty_lists_empty_result() {
        let empty: Vec<Vec<&str>> = vec![vec![], vec![]];
        let fused = rrf_fuse(empty, 60.0, 10);
        assert!(fused.is_empty());
    }
}
