//! Deterministic, compactor-independent context floor.
//!
//! `truncate_to_fit` is the last line of the never-break guarantee: even when
//! the LLM compactor is unwired or its summary still overflows, this pure
//! function guarantees the working message list fits the target token budget by
//! dropping the oldest non-tail messages. Zero LLM calls, fully deterministic.

use crate::context::budget::pressure::estimate_message_tokens_aware;
use crate::context::budget::ContextBudget;
use crate::context::compact::compactor::ContextCompactor;
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
/// Tool-pair safety: after dropping from the front, any leading orphaned
/// `tool_result` (whose `tool_use` was dropped) is snapped away, so the
/// surviving list never begins with an orphan.
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
    // Snap forward past any leading orphaned tool_result whose paired tool_use
    // was just dropped. A message list must never begin with a tool_result —
    // Anthropic-compatible backends reject an orphan with HTTP 400. Dropping
    // these only frees more budget, so the fit post-condition still holds, and
    // `tail` is still respected so the protected tail survives.
    while messages.len() > tail && messages.first().is_some_and(UnifiedMessage::is_tool_result) {
        messages.remove(0);
    }
    before.saturating_sub(estimate_total(messages, prose_ratio))
}

/// Shrink the working message list toward fitting under the budget's critical
/// line, compacting as gently as possible: (1) try the LLM compactor if wired,
/// (2) re-measure, (3) if still critical, apply the deterministic
/// `truncate_to_fit` floor. Post-condition: the returned list's pressure ratio
/// is below `critical_threshold` UNLESS the protected fresh tail plus the fixed
/// overhead (system prompt + tool schemas) alone already meet/exceed the budget
/// — the Plan-1b pathological case, where the caller continues anyway. Never
/// returns an error and never hard-stops — this IS the never-break mechanism.
pub async fn compact_to_fit(
    compactor: Option<&ContextCompactor>,
    budget: &ContextBudget,
    messages: &mut Vec<UnifiedMessage>,
    system_prompt: &str,
    tool_schema_tokens: usize,
    session_id: Option<&str>,
) {
    let critical = budget.critical_threshold();
    let ratio = budget.token_estimate_ratio();

    // 1. LLM compaction (aggressive: minimal fresh tail). Fail-soft.
    if let Some(c) = compactor {
        if let Err(e) = c
            .compact(messages, budget.fresh_tail_count(), session_id)
            .await
        {
            tracing::warn!(error = %e, "compact_to_fit: LLM compaction failed; falling back to floor");
        }
    }

    // 2. Re-measure.
    let p = budget.peek_pressure(messages, system_prompt, tool_schema_tokens);
    if p.ratio < critical {
        return;
    }

    // 3. Deterministic floor. Target = critical fraction of the message budget,
    //    minus the fixed overhead (system + tools) so the floor accounts for
    //    what the LLM call will actually carry.
    let budget_tokens = budget.token_budget() as usize;
    let overhead = p.overhead_tokens;
    let target = ((budget_tokens as f64 * critical) as usize)
        .saturating_sub(overhead)
        .saturating_sub(1); // strict: guarantee ratio < critical, not <=
    truncate_to_fit(messages, target, budget.fresh_tail_count(), ratio);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::UnifiedMessage;

    fn text_user(s: &str) -> UnifiedMessage {
        UnifiedMessage::user(s.to_string())
    }

    fn total(msgs: &[UnifiedMessage], ratio: f64) -> usize {
        msgs.iter()
            .map(|m| estimate_message_tokens_aware(m, ratio))
            .sum()
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
            Some(estimate_message_tokens_aware(
                &text_user(&"c".repeat(400)),
                3.5
            ))
        );
    }

    #[test]
    fn never_drops_below_protected_tail() {
        let mut msgs = vec![text_user(&"a".repeat(4000)), text_user("keep me")];
        truncate_to_fit(&mut msgs, 1, 1, 3.5); // absurdly small target
        assert!(!msgs.is_empty(), "protected tail must survive");
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn floor_never_leaves_leading_orphan_tool_result() {
        // Dropping the big assistant message leaves [tool_result, user] under
        // target; without the snap, the tool_result would lead (orphan).
        let mut msgs = vec![
            UnifiedMessage::assistant("a".repeat(4000)),
            UnifiedMessage::tool_result("call_1", "some_tool", "ok", false),
            UnifiedMessage::user("tail"),
        ];
        truncate_to_fit(&mut msgs, 200, 1, 3.5);
        assert!(
            !msgs.first().unwrap().is_tool_result(),
            "surviving list must not begin with an orphaned tool_result"
        );
    }

    #[tokio::test]
    async fn guarantees_fit_via_floor_when_no_compactor() {
        use crate::context::budget::{ContextBudget, ContextBudgetConfig};
        let config = ContextBudgetConfig {
            token_budget: 1000,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            token_estimate_ratio: 3.5,
            fresh_tail_count: 1,
            circuit_breaker_max: 2,
            diminishing_window: 3,
            diminishing_threshold: 100,
            max_splits: 3,
        };
        let budget = ContextBudget::new(&config);
        let mut msgs = vec![
            text_user(&"a".repeat(20000)), // way over 0.85*1000 tokens
            text_user(&"b".repeat(20000)),
            text_user("tail"),
        ];
        compact_to_fit(None, &budget, &mut msgs, "", 0, None).await;
        let p = budget.peek_pressure(&msgs, "", 0);
        assert!(
            p.ratio < 0.85,
            "post-condition: pressure must be under critical, got {}",
            p.ratio
        );
    }
}
