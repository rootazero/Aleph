//! Deterministic, compactor-independent context floor.
//!
//! `truncate_to_fit` is the last line of the never-break guarantee: even when
//! the LLM compactor is unwired or its summary still overflows, this pure
//! function guarantees the working message list fits the target token budget by
//! dropping the oldest non-tail messages. Zero LLM calls, fully deterministic.

use crate::context::budget::pressure::estimate_message_tokens_aware;
use crate::providers::message::UnifiedMessage;

/// Estimate the total token footprint of `messages`.
fn estimate_total(messages: &[UnifiedMessage], prose_ratio: f64) -> usize {
    messages
        .iter()
        .map(|m| estimate_message_tokens_aware(m, prose_ratio))
        .sum()
}

/// Drop oldest non-tail messages until the estimated footprint fits
/// `target_tokens`. Always preserves at least the last `protected_tail`
/// messages. Returns the estimated tokens dropped.
///
/// Tool-pair safety: dropping proceeds from the front one message at a time and
/// stops at the protected-tail boundary, so a surviving `ToolResult` can never
/// be orphaned from its `ToolCall` (both are in the protected tail or both are
/// dropped together as the front advances).
pub fn truncate_to_fit(
    messages: &mut Vec<UnifiedMessage>,
    target_tokens: usize,
    protected_tail: usize,
    prose_ratio: f64,
) -> usize {
    let before = estimate_total(messages, prose_ratio);
    let tail = protected_tail.max(1);
    while messages.len() > tail && estimate_total(messages, prose_ratio) > target_tokens {
        messages.remove(0);
    }
    before.saturating_sub(estimate_total(messages, prose_ratio))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::UnifiedMessage;

    fn text_user(s: &str) -> UnifiedMessage {
        UnifiedMessage::user(s.to_string())
    }

    fn total(msgs: &[UnifiedMessage], ratio: f64) -> usize {
        msgs.iter().map(|m| estimate_message_tokens_aware(m, ratio)).sum()
    }

    #[test]
    fn drops_oldest_until_under_target_keeping_tail() {
        let mut msgs = vec![
            text_user(&"a".repeat(4000)),
            text_user(&"b".repeat(4000)),
            text_user(&"c".repeat(400)), // fresh tail
        ];
        let before = total(&msgs, 3.5);
        let dropped = truncate_to_fit(&mut msgs, before / 3, 1, 3.5);
        assert!(dropped > 0);
        assert!(total(&msgs, 3.5) <= before / 3, "must fit under target");
        // fresh tail (last message) preserved
        assert_eq!(
            msgs.last().map(|m| estimate_message_tokens_aware(m, 3.5)),
            Some(estimate_message_tokens_aware(&text_user(&"c".repeat(400)), 3.5))
        );
    }

    #[test]
    fn never_drops_below_protected_tail() {
        let mut msgs = vec![text_user(&"a".repeat(4000)), text_user("keep me")];
        truncate_to_fit(&mut msgs, 1, 1, 3.5); // absurdly small target
        assert!(!msgs.is_empty(), "protected tail must survive");
        assert_eq!(msgs.len(), 1);
    }
}
