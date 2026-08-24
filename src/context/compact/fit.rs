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

/// Drop non-tail messages until the estimated footprint fits `target_tokens`.
/// Always preserves at least the last `protected_tail` messages. Returns the
/// estimated tokens dropped.
///
/// Role-aware (B13): assistant turns and tool results are evicted before
/// anything the user said. A blind `remove(0)` deleted the user's original
/// instruction first — and, inside `compact_to_fit` (where the compactor runs
/// first), its very first `remove(0)` deleted the `[Context Summary]` a
/// side-channel LLM call had just been paid for, since the summary is inserted
/// as a user message at the head. User-role messages are the last thing to go,
/// and only when nothing else remains outside the protected tail.
///
/// Tool-pair safety: an assistant `tool_use` is evicted TOGETHER WITH the
/// `tool_result` that answers it, because a split pair is wrong in both
/// directions. Drop the result and keep the call, and
/// `providers::message::ensure_tool_results_present` synthesises an
/// `is_error = true` "No result provided — tool call was interrupted" answer —
/// a fabricated tool failure that also costs more tokens than the result it
/// replaced. Drop the call and keep the result, and `remove_orphan_tool_results`
/// strips the result at the wire — so the floor's estimate counted tokens that
/// never ship, and the model loses the result with no account of it. After
/// dropping from the front, any leading orphaned `tool_result` is still snapped
/// away, so the surviving list never begins with an orphan.
pub(crate) fn truncate_to_fit(
    messages: &mut Vec<UnifiedMessage>,
    target_tokens: usize,
    protected_tail: usize,
    prose_ratio: f64,
) -> usize {
    let before = estimate_total(messages, prose_ratio);
    let tail = protected_tail.max(1);
    while messages.len() > tail && estimate_total(messages, prose_ratio) > target_tokens {
        evict_one(messages, tail);
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

/// Evict exactly one eviction *unit* from the removable region
/// (`0..len - tail`), oldest-first among non-user messages. Caller guarantees
/// `messages.len() > tail`, so this always removes at least one message and the
/// fit loop always terminates.
///
/// The victim is the oldest assistant/tool message; when nothing but user
/// messages remains outside the tail, it falls back to the oldest message
/// whatever its role — the fit post-condition outranks role preference.
///
/// Scanning front-to-back is what keeps the fabricating half of the trap in
/// [`truncate_to_fit`] unreachable: a result always follows its call, so the
/// first non-user message can only be a result when that result is ALREADY an
/// orphan. An assistant carrying `tool_use` blocks takes the results answering
/// them along, bounded to the removable region — a result stranded in the
/// protected tail is left to the wire-level orphan strip rather than eaten out
/// of the tail the caller asked us to keep.
fn evict_one(messages: &mut Vec<UnifiedMessage>, tail: usize) {
    let removable_end = messages.len() - tail;
    let victim = (0..removable_end)
        .find(|&i| !matches!(messages[i], UnifiedMessage::User { .. }))
        .unwrap_or(0);

    let answered: Vec<String> = messages[victim]
        .tool_calls()
        .iter()
        .map(|(id, _, _)| (*id).to_string())
        .collect();
    messages.remove(victim);

    while messages.len() > tail
        && victim < messages.len() - tail
        && matches!(&messages[victim], UnifiedMessage::ToolResult { tool_call_id, .. }
            if answered.iter().any(|id| id == tool_call_id))
    {
        messages.remove(victim);
    }
}

/// Shrink the working message list toward fitting under the budget's critical
/// line, compacting as gently as possible: (1) try the LLM compactor if wired,
/// (2) re-measure, (3) if still critical, apply the deterministic
/// `truncate_to_fit` floor. Post-condition: the returned list's pressure ratio
/// is below `critical_threshold` UNLESS the protected fresh tail plus the fixed
/// overhead (system prompt + tool schemas) alone already meet/exceed the budget
/// — the Plan-1b pathological case, where the caller continues anyway. Never
/// returns an error and never hard-stops — this IS the never-break mechanism.
pub(crate) async fn compact_to_fit(
    compactor: Option<&ContextCompactor>,
    budget: &ContextBudget,
    messages: &mut Vec<UnifiedMessage>,
    system_prompt: &str,
    tool_schema_tokens: usize,
    transient_tail: usize,
    session_id: Option<&str>,
) {
    let critical = budget.critical_threshold();
    let ratio = budget.token_estimate_ratio();

    // 1. LLM compaction (aggressive: minimal fresh tail). Fail-soft.
    if let Some(c) = compactor {
        if let Err(e) = c
            .compact(
                messages,
                budget.fresh_tail_count(),
                transient_tail,
                session_id,
            )
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
    //    what the LLM call will actually carry. `p` (and the post-condition
    //    ratio) live in CALIBRATED space — `peek_pressure` applies the EWMA
    //    factor — while `truncate_to_fit`'s eviction loop measures RAW
    //    estimates, so the target must be divided back into raw space:
    //    post-condition f·(raw_msgs + raw_overhead) < critical·budget with
    //    p.overhead_tokens = f·raw_overhead gives
    //    raw_msgs < (critical·budget − p.overhead_tokens) / f. With f > 1 a
    //    calibrated-space target stopped evicting while the calibrated ratio
    //    was still ≥ critical, pinning `before_turn` on CompactToFit forever.
    //    The budget clamps f to [0.25, 4.0]; guard non-finite anyway. At
    //    f = 1.0 (uncalibrated) this is byte-identical to the old arithmetic.
    let budget_tokens = budget.token_budget() as usize;
    let factor = match budget.calibration() {
        Some(f) if f.is_finite() && f > 0.0 => f,
        _ => 1.0,
    };
    let headroom = (budget_tokens as f64 * critical) - p.overhead_tokens as f64;
    // `- 1` keeps it strict: guarantee ratio < critical, not <=.
    let target = ((headroom / factor).floor() as usize).saturating_sub(1);
    // `+ transient_tail` for the same reason the compactor above gets it: the
    // floor's `protected_tail` counts from the END of a vector whose last few
    // entries are synthetic nudges, so a bare `fresh_tail_count()` spends the
    // conversation's protection budget on messages that were never part of the
    // conversation. Adding it can only make the floor evict LESS, so the fit
    // post-condition is unchanged — `truncate_to_fit` still stops at `tail`
    // and the caller continues regardless (the Plan-1b pathological case).
    truncate_to_fit(
        messages,
        target,
        budget.fresh_tail_count().saturating_add(transient_tail),
        ratio,
    );
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
    fn floor_evicts_assistant_and_tool_history_before_the_users_own_turns() {
        // B13: the user's instruction is the FIRST thing a blind `remove(0)`
        // deleted. Assistant/tool history is reconstructible from the summary;
        // what the user asked for is not.
        let mut msgs = vec![
            text_user("ORIGINAL: migrate the store, keep the API stable"),
            UnifiedMessage::assistant("a".repeat(4000)),
            UnifiedMessage::tool_result("c1", "file_read", "b".repeat(4000), false),
            text_user("tail"),
        ];
        let before = total(&msgs, 3.5);
        truncate_to_fit(&mut msgs, before / 4, 1, 3.5);
        assert!(
            total(&msgs, 3.5) <= before / 4,
            "fit post-condition still holds"
        );
        assert_eq!(
            msgs.first().map(UnifiedMessage::text_content),
            Some("ORIGINAL: migrate the store, keep the API stable".to_string()),
            "assistant/tool history must be evicted before the user's own turn"
        );
    }

    #[test]
    fn floor_keeps_the_summary_the_compactor_just_paid_for() {
        // Inside `compact_to_fit` the compactor runs FIRST, so by the time the
        // floor evicts, messages[0] is the `[Context Summary]` a side-channel
        // LLM call was just billed for. The old floor deleted it immediately.
        let mut msgs = vec![
            UnifiedMessage::user("[Context Summary]\nwhat happened so far"),
            UnifiedMessage::assistant("a".repeat(8000)),
            text_user("tail"),
        ];
        let before = total(&msgs, 3.5);
        truncate_to_fit(&mut msgs, before / 4, 1, 3.5);
        assert!(msgs
            .iter()
            .any(|m| m.text_content().starts_with("[Context Summary]")));
    }

    #[test]
    fn floor_evicts_a_tool_call_together_with_its_result() {
        // A split pair is wrong in both directions: drop the result and
        // `ensure_tool_results_present` FABRICATES an `is_error = true` "tool
        // call was interrupted" answer; drop the call and the result is stranded
        // as an orphan that only gets stripped at the wire. Evict them as one
        // unit. Here the bloat lives in the CALL (a huge argument payload), so
        // evicting it alone already satisfies the target — a floor without the
        // unit rule stops right there and leaves the result behind.
        use crate::providers::message::ContentBlock;
        let mut msgs = vec![
            text_user("keep me"),
            UnifiedMessage::Assistant {
                content: vec![ContentBlock::ToolCall {
                    thought_signature: None,
                    id: "pair".into(),
                    name: "search".into(),
                    arguments: serde_json::json!({ "q": "x".repeat(8000) }),
                }],
            },
            UnifiedMessage::tool_result("pair", "search", "ok", false),
            text_user("tail"),
        ];
        truncate_to_fit(&mut msgs, 100, 1, 3.5);

        let calls: Vec<String> = msgs
            .iter()
            .flat_map(UnifiedMessage::tool_calls)
            .map(|(id, _, _)| id.to_string())
            .collect();
        let results: Vec<String> = msgs
            .iter()
            .filter_map(|m| match m {
                UnifiedMessage::ToolResult { tool_call_id, .. } => Some(tool_call_id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            calls, results,
            "call and result must be evicted as one unit, never split"
        );
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
    async fn floor_converts_target_into_raw_space_when_calibrated() {
        use crate::context::budget::{ContextBudget, ContextBudgetConfig};
        let config = ContextBudgetConfig {
            token_budget: 1000,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            token_estimate_ratio: 1.0,
            fresh_tail_count: 1,
            summarizer_input_budget: 48_000,
            circuit_breaker_max: 2,
            max_splits: 3,
        };
        let mut budget = ContextBudget::new(&config);
        // Push the EWMA calibration factor to exactly 2.0: the provider
        // reports twice the raw estimate on the first observation.
        let seed = vec![text_user(&"s".repeat(500))];
        budget.before_turn(&seed, "", 0);
        let estimated = budget.last_pressure().unwrap().used_tokens;
        budget.observe_actual_usage(estimated * 2);
        assert!((budget.calibration().unwrap() - 2.0).abs() < 1e-9);

        // Raw total ~804 tokens: UNDER the calibrated-space floor target
        // (0.85·1000 − 1 = 849 raw) but 2×-calibrated far ABOVE critical
        // (1608 ≥ 850). A floor comparing raw estimates against that
        // calibrated-space target evicts nothing here and leaves the
        // calibrated ratio pinned ≥ critical forever.
        let mut msgs: Vec<UnifiedMessage> = (0..8).map(|_| text_user(&"a".repeat(100))).collect();
        msgs.push(text_user("tail"));
        let raw = total(&msgs, 1.0);
        assert!(
            (425..=849).contains(&raw),
            "premise: raw total must sit between the raw and calibrated targets, got {raw}"
        );

        compact_to_fit(None, &budget, &mut msgs, "", 0, 0, None).await;
        let p = budget.peek_pressure(&msgs, "", 0);
        assert!(
            p.ratio < 0.85,
            "post-condition must hold in CALIBRATED space, got {}",
            p.ratio
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
            summarizer_input_budget: 48_000,
            circuit_breaker_max: 2,
            max_splits: 3,
        };
        let budget = ContextBudget::new(&config);
        let mut msgs = vec![
            text_user(&"a".repeat(20000)), // way over 0.85*1000 tokens
            text_user(&"b".repeat(20000)),
            text_user("tail"),
        ];
        compact_to_fit(None, &budget, &mut msgs, "", 0, 0, None).await;
        let p = budget.peek_pressure(&msgs, "", 0);
        assert!(
            p.ratio < 0.85,
            "post-condition: pressure must be under critical, got {}",
            p.ratio
        );
    }
}
