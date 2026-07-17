//! Context window measurement helpers for the session compactor.
//!
//! Token estimation (forwarding to the single-source estimator) and
//! fresh-tail partitioning used by `prepare_history` / `post_turn_compress`.

/// Estimate token count for `content`, treating `ratio` as the prose
/// (chars-per-token) anchor. Thin forwarding shell over
/// [`crate::context::budget::pressure::estimate_tokens_aware`] — the single
/// source of truth — so CJK / code content is charged at its denser ratio
/// while the caller-supplied anchor still governs ordinary prose.
#[must_use]
pub fn estimate_tokens(content: &str, ratio: f64) -> usize {
    crate::context::budget::pressure::estimate_tokens_aware(content, ratio)
}

/// Partition `(role, content)` string pairs into (compressible, `fresh_tail`).
///
/// Returns the index where `fresh_tail` begins. Pairs in `[0..idx]` are
/// candidates for compression; pairs in `[idx..]` are kept verbatim.
#[must_use]
pub const fn partition_fresh_tail_pairs(
    messages: &[(String, String)],
    fresh_tail_count: usize,
) -> usize {
    if messages.len() <= fresh_tail_count {
        0
    } else {
        messages.len() - fresh_tail_count
    }
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;

    // --- estimate_tokens ---

    #[test]
    fn test_estimate_tokens_english() {
        let content = "Hello world, this is a test message for token estimation.";
        let tokens = estimate_tokens(content, 3.5);
        assert!(tokens > 10 && tokens < 25, "got {tokens}");
    }

    #[test]
    fn test_estimate_tokens_empty() {
        assert_eq!(estimate_tokens("", 3.5), 0);
    }

    #[test]
    fn test_estimate_tokens_zero_ratio_returns_zero() {
        assert_eq!(estimate_tokens("hello world", 0.0), 0);
    }

    #[test]
    fn test_estimate_tokens_ratio_one() {
        // ratio=1.0 => tokens == char count
        let s = "abcde";
        assert_eq!(estimate_tokens(s, 1.0), 5);
    }

    #[test]
    fn test_estimate_tokens_counts_chars_not_bytes() {
        // 5 CJK characters = 15 UTF-8 bytes. The estimate must be driven by
        // the 5 characters, not the 15 bytes — and by default, CJK is charged
        // at the denser CJK_RATIO (1.5) regardless of the prose_ratio anchor.
        // So 5 chars / 1.5 = 3.333... ≈ 4 tokens.
        let cjk = "你好世界呀";
        assert_eq!(cjk.chars().count(), 5);
        assert_eq!(cjk.len(), 15);
        assert_eq!(estimate_tokens(cjk, 1.0), 4);
    }

    #[test]
    fn estimate_tokens_charges_cjk_at_dense_ratio() {
        // ratio 参数仍是散文锚点；CJK 内容由单源以 ~1.5 chars/token 计。
        let cjk = "忆".repeat(50);
        let tokens = estimate_tokens(&cjk, 3.5);
        assert!(tokens >= 30, "expected dense CJK charge, got {tokens}");
    }

    // --- partition_fresh_tail_pairs ---

    #[test]
    fn test_partition_fresh_tail_pairs_normal() {
        let pairs: Vec<(String, String)> = (0..30)
            .map(|i| ("user".to_string(), format!("msg {i}")))
            .collect();
        let split = partition_fresh_tail_pairs(&pairs, 10);
        assert_eq!(split, 20);
    }

    #[test]
    fn test_partition_fresh_tail_pairs_short() {
        let pairs: Vec<(String, String)> = (0..5)
            .map(|i| ("user".to_string(), format!("msg {i}")))
            .collect();
        let split = partition_fresh_tail_pairs(&pairs, 10);
        assert_eq!(split, 0);
    }
}
