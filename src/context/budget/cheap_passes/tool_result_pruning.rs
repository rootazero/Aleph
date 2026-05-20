//! `ToolResultPruningStage` — replace stale large tool_results with one-line
//! placeholders to save tokens before the LLM compactor runs.
//!
//! Hermes-borrowed heuristic: tool results outside the fresh tail rarely add
//! value verbatim once the agent has moved on; a placeholder preserves the
//! "this tool was called" signal at a fraction of the token cost.

use crate::context::budget::pressure::estimate_tokens_smart;
use crate::context::budget::ContextPressure;
use crate::providers::message::UnifiedMessage;
use async_trait::async_trait;

/// Cheap-pass stage that shortens stale `ToolResult` messages.
///
/// Keeps the newest `fresh_tail_count` messages untouched. For older
/// `ToolResult` blocks above `min_tokens_to_prune`, replaces the content
/// with `"[pruned tool_result: <tool_name>, ~<N> tokens]"`. Skips when
/// the placeholder wouldn't actually save tokens.
pub struct ToolResultPruningStage {
    /// Minimum token size before pruning kicks in. Tool results smaller
    /// than this are kept verbatim (the placeholder itself costs tokens).
    pub min_tokens_to_prune: usize,
}

impl Default for ToolResultPruningStage {
    fn default() -> Self {
        Self {
            min_tokens_to_prune: 200,
        }
    }
}

#[async_trait]
impl crate::context::budget::preflight::PreflightStage for ToolResultPruningStage {
    fn name(&self) -> &'static str {
        "tool_result_pruning"
    }

    async fn prepare(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        _pressure: &ContextPressure,
        fresh_tail_count: usize,
    ) -> usize {
        if messages.len() <= fresh_tail_count {
            return 0;
        }
        let cut_end = messages.len() - fresh_tail_count;
        let mut total_freed: usize = 0;

        for msg in messages.iter_mut().take(cut_end) {
            // tool_result_info returns (tool_name, joined_text) for ToolResults,
            // None otherwise. Cheap match-and-extract that doesn't require us
            // to know ContentBlock's internal shape.
            let Some((tool_name, original_text)) = msg.tool_result_info() else {
                continue;
            };
            let original_tokens = estimate_tokens_smart(&original_text);
            if original_tokens < self.min_tokens_to_prune {
                continue;
            }
            let tool_name_owned = tool_name.to_string();
            let placeholder =
                format!("[pruned tool_result: {tool_name_owned}, ~{original_tokens} tokens]");
            let new_tokens = estimate_tokens_smart(&placeholder);
            if new_tokens >= original_tokens {
                continue;
            }
            msg.replace_tool_result_content(placeholder);
            total_freed += original_tokens - new_tokens;
        }
        total_freed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::budget::preflight::PreflightStage;
    use crate::context::budget::ContextPressure;
    use crate::providers::message::UnifiedMessage;

    fn make_pressure() -> ContextPressure {
        ContextPressure {
            used_tokens: 5000,
            budget_tokens: 10000,
            ratio: 0.5,
            overhead_tokens: 0,
            available_for_messages: 5000,
        }
    }

    fn big_tool_result() -> UnifiedMessage {
        UnifiedMessage::tool_result("call-1", "Read", "x".repeat(2000), false)
    }

    fn small_tool_result() -> UnifiedMessage {
        UnifiedMessage::tool_result("call-2", "Read", "short output", false)
    }

    #[tokio::test]
    async fn prunes_old_large_tool_result() {
        let mut messages = vec![
            big_tool_result(),
            UnifiedMessage::user("recent 1"),
            UnifiedMessage::user("recent 2"),
            UnifiedMessage::user("recent 3"),
        ];
        let stage = ToolResultPruningStage::default();
        let freed = stage.prepare(&mut messages, &make_pressure(), 3).await;
        assert!(freed > 100, "expected significant savings, got {freed}");
        let (_name, text) = messages[0].tool_result_info().expect("still a ToolResult");
        assert!(
            text.starts_with("[pruned tool_result"),
            "got: {text}"
        );
    }

    #[tokio::test]
    async fn skips_small_tool_result() {
        let mut messages = vec![small_tool_result(), UnifiedMessage::user("recent")];
        let stage = ToolResultPruningStage::default();
        let freed = stage.prepare(&mut messages, &make_pressure(), 1).await;
        assert_eq!(freed, 0, "small results must not be pruned");
        let (_name, text) = messages[0].tool_result_info().expect("still a ToolResult");
        assert_eq!(text, "short output");
    }

    #[tokio::test]
    async fn protects_fresh_tail() {
        let mut messages = vec![
            UnifiedMessage::user("oldest"),
            big_tool_result(), // sits in protected tail when fresh_tail=2
        ];
        let stage = ToolResultPruningStage::default();
        let freed = stage.prepare(&mut messages, &make_pressure(), 2).await;
        assert_eq!(freed, 0, "fresh tail must be inviolable");
        let (_name, text) = messages[1].tool_result_info().expect("still a ToolResult");
        assert!(
            text.starts_with("xxx"),
            "tail tool_result must remain verbatim; got: {}",
            text.chars().take(20).collect::<String>()
        );
    }

    #[tokio::test]
    async fn empty_messages_no_op() {
        let mut messages: Vec<UnifiedMessage> = vec![];
        let stage = ToolResultPruningStage::default();
        let freed = stage.prepare(&mut messages, &make_pressure(), 3).await;
        assert_eq!(freed, 0);
    }

    #[tokio::test]
    async fn non_tool_messages_untouched() {
        let mut messages = vec![
            UnifiedMessage::user("a".repeat(2000)),
            UnifiedMessage::assistant("b".repeat(2000)),
            UnifiedMessage::user("recent"),
        ];
        let stage = ToolResultPruningStage::default();
        let freed = stage.prepare(&mut messages, &make_pressure(), 1).await;
        assert_eq!(freed, 0, "non-ToolResult messages must not be pruned");
    }
}
