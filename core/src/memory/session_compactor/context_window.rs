//! Context window measurement and threshold detection.
//!
//! Estimates current token usage and determines when compaction should be
//! triggered based on the configured `context_threshold`.

use crate::providers::message::UnifiedMessage;

/// Estimate token count for a string using a char-length heuristic.
///
/// `ratio` is the average number of characters per token (e.g. 3.5 for English).
pub fn estimate_tokens(content: &str, ratio: f64) -> usize {
    if ratio <= 0.0 {
        return 0;
    }
    (content.len() as f64 / ratio) as usize
}

/// Estimate total tokens across all messages.
pub fn estimate_total_tokens(messages: &[UnifiedMessage], ratio: f64) -> usize {
    messages
        .iter()
        .map(|m| estimate_tokens(&m.text_content(), ratio))
        .sum()
}

/// Partition messages into (compressible, fresh_tail).
///
/// Returns the index where `fresh_tail` begins. Messages in `[0..idx]` are
/// candidates for compression; messages in `[idx..]` are kept verbatim.
pub fn partition_fresh_tail(messages: &[UnifiedMessage], fresh_tail_count: usize) -> usize {
    if messages.len() <= fresh_tail_count {
        0
    } else {
        messages.len() - fresh_tail_count
    }
}

/// Same as [`partition_fresh_tail`] but for `(role, content)` string pairs.
pub fn partition_fresh_tail_pairs(
    messages: &[(String, String)],
    fresh_tail_count: usize,
) -> usize {
    if messages.len() <= fresh_tail_count {
        0
    } else {
        messages.len() - fresh_tail_count
    }
}

/// Check if a tool result at position `idx` has been "consumed" —
/// i.e. an assistant message exists after it in the list.
///
/// An unconsumed tool result means the LLM has not yet seen the result, so it
/// must not be compacted away.
pub fn is_tool_result_consumed(messages: &[UnifiedMessage], idx: usize) -> bool {
    for i in (idx + 1)..messages.len() {
        if messages[i].is_assistant() {
            return true;
        }
    }
    false
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

    // --- estimate_total_tokens ---

    #[test]
    fn test_estimate_total_tokens_empty_slice() {
        assert_eq!(estimate_total_tokens(&[], 3.5), 0);
    }

    #[test]
    fn test_estimate_total_tokens_sums_messages() {
        let messages = vec![
            UnifiedMessage::user("hello world"),
            UnifiedMessage::assistant("goodbye world"),
        ];
        let expected = estimate_tokens("hello world", 4.0)
            + estimate_tokens("goodbye world", 4.0);
        assert_eq!(estimate_total_tokens(&messages, 4.0), expected);
    }

    // --- partition_fresh_tail ---

    #[test]
    fn test_partition_fresh_tail_normal() {
        let messages: Vec<UnifiedMessage> = (0..50)
            .map(|i| UnifiedMessage::user(format!("msg {i}")))
            .collect();
        let split = partition_fresh_tail(&messages, 20);
        assert_eq!(split, 30);
    }

    #[test]
    fn test_partition_fresh_tail_short_history() {
        let messages: Vec<UnifiedMessage> = (0..5)
            .map(|i| UnifiedMessage::user(format!("msg {i}")))
            .collect();
        let split = partition_fresh_tail(&messages, 20);
        assert_eq!(split, 0);
    }

    #[test]
    fn test_partition_fresh_tail_exact_boundary() {
        let messages: Vec<UnifiedMessage> = (0..20)
            .map(|i| UnifiedMessage::user(format!("msg {i}")))
            .collect();
        // exactly fresh_tail_count messages → all are fresh
        let split = partition_fresh_tail(&messages, 20);
        assert_eq!(split, 0);
    }

    #[test]
    fn test_partition_fresh_tail_one_more() {
        let messages: Vec<UnifiedMessage> = (0..21)
            .map(|i| UnifiedMessage::user(format!("msg {i}")))
            .collect();
        let split = partition_fresh_tail(&messages, 20);
        assert_eq!(split, 1);
    }

    #[test]
    fn test_partition_fresh_tail_zero_fresh() {
        let messages: Vec<UnifiedMessage> = (0..10)
            .map(|i| UnifiedMessage::user(format!("msg {i}")))
            .collect();
        let split = partition_fresh_tail(&messages, 0);
        assert_eq!(split, 10);
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

    // --- is_tool_result_consumed ---

    #[test]
    fn test_tool_result_consumed_when_assistant_follows() {
        let messages = vec![
            UnifiedMessage::user("search something"),
            UnifiedMessage::tool_result("c1", "search", "results here", false),
            UnifiedMessage::assistant("I found the results."),
        ];
        // tool result is at index 1, assistant follows at index 2
        assert!(is_tool_result_consumed(&messages, 1));
    }

    #[test]
    fn test_tool_result_not_consumed_when_no_assistant_follows() {
        let messages = vec![
            UnifiedMessage::user("search something"),
            UnifiedMessage::tool_result("c1", "search", "results here", false),
        ];
        // no assistant message after index 1
        assert!(!is_tool_result_consumed(&messages, 1));
    }

    #[test]
    fn test_tool_result_not_consumed_at_last_position() {
        let messages = vec![
            UnifiedMessage::user("hi"),
            UnifiedMessage::assistant("response"),
            UnifiedMessage::tool_result("c2", "tool", "output", false),
        ];
        assert!(!is_tool_result_consumed(&messages, 2));
    }

    #[test]
    fn test_tool_result_consumed_skips_non_assistant_in_between() {
        let messages = vec![
            UnifiedMessage::tool_result("c1", "search", "out", false),
            UnifiedMessage::tool_result("c2", "other", "out2", false),
            UnifiedMessage::assistant("done"),
        ];
        // idx=0: assistant at 2 → consumed
        assert!(is_tool_result_consumed(&messages, 0));
        // idx=1: assistant at 2 → consumed
        assert!(is_tool_result_consumed(&messages, 1));
    }

    #[test]
    fn test_tool_result_consumed_empty_tail() {
        let messages: Vec<UnifiedMessage> = vec![
            UnifiedMessage::tool_result("c1", "t", "o", false),
        ];
        assert!(!is_tool_result_consumed(&messages, 0));
    }
}
