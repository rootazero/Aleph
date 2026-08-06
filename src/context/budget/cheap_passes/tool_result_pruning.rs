//! `ToolResultPruningStage` — replace stale large `tool_results` with one-line
//! placeholders to save tokens before the LLM compactor runs.
//!
//! Hermes-borrowed heuristic: tool results outside the fresh tail rarely add
//! value verbatim once the agent has moved on; an *informative* placeholder
//! preserves the "what did this tool do" signal at a fraction of the token
//! cost — far better continuity than a bare token count.

use crate::context::budget::pressure::estimate_tokens_smart;
use crate::context::budget::ContextPressure;
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::tool_output::sanitize::sanitize_command_output;
use crate::tool_output::structured;
use async_trait::async_trait;

/// Max characters kept from a tool result's first line in the pruned hint.
const MAX_HINT_CHARS: usize = 120;

/// Build a one-line informative hint from a tool result body: the first
/// non-empty line (char-capped at [`MAX_HINT_CHARS`]) plus a line count when
/// the result spans multiple lines. This is the hermes touch — `exit 0,
/// 47 lines` carries continuity a bare token count throws away.
fn result_hint(text: &str) -> String {
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    // Char-safe truncation (P7 UTF-8 safety): never slice on a byte boundary.
    let excerpt: String = first.chars().take(MAX_HINT_CHARS).collect();
    let truncated = first.chars().count() > MAX_HINT_CHARS;
    let lines = text.lines().count();
    match (truncated, lines) {
        (true, n) if n > 1 => format!("{excerpt}… ({n} lines)"),
        (true, _) => format!("{excerpt}…"),
        (false, n) if n > 1 => format!("{excerpt} ({n} lines)"),
        (false, _) => excerpt,
    }
}

/// Build the first-line placeholder for unstructured content (the fallback when
/// no [`structured`] reducer matches). Preserves the original
/// `[pruned tool_result: …]` shape so downstream conventions stay intact.
fn first_line_placeholder(tool_name: &str, original_tokens: usize, text: &str) -> String {
    let hint = result_hint(text);
    if hint.is_empty() {
        format!("[pruned tool_result: {tool_name}, ~{original_tokens} tokens]")
    } else {
        format!("[pruned tool_result: {tool_name}, ~{original_tokens} tokens — {hint}]")
    }
}

/// Cheap-pass stage that shortens stale `ToolResult` messages.
///
/// Keeps the newest `fresh_tail_count` messages untouched. For older
/// `ToolResult` blocks above `min_tokens_to_prune`, replaces the content with
/// `"[pruned tool_result: <tool_name>, ~<N> tokens — <hint>]"`. Skips when the
/// placeholder wouldn't actually save tokens.
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
        let cut_end = messages.len().saturating_sub(fresh_tail_count);
        let mut total_freed: usize = 0;

        for msg in messages.iter_mut().take(cut_end) {
            // tool_result_info returns (tool_name, joined_text) for ToolResults,
            // None otherwise. Cheap match-and-extract that doesn't require us
            // to know ContentBlock's internal shape.
            let Some((tool_name, original_text)) = msg.tool_result_info() else {
                continue;
            };
            // Already-persisted markers (Layer 2 of the tool-result budget
            // produced by `tools::result_processing::apply_result_budget`)
            // are already compact and carry the disk path the LLM needs.
            // Re-pruning them would erase the recovery handle.
            //
            // The marker is **not** necessarily at byte 0: Layer 2 puts an
            // inline error digest, or the content-reduced body, *above* it. A
            // `starts_with` test therefore missed the entire hygiene path and
            // pruned exactly the results whose recovery handle matters most —
            // the reduced body would be replaced by a first-line placeholder and
            // the offloaded blob became unreachable. Scan every line, through
            // the same single-source predicate `act.rs` already uses.
            if crate::tools::result_store::extract_persisted_ref(&original_text).is_some() {
                continue;
            }
            let original_tokens = estimate_tokens_smart(&original_text);
            if original_tokens < self.min_tokens_to_prune {
                continue;
            }
            // Share the ingress cleaner's preprocessing. Hygiene strips ANSI
            // before classifying because an escape run ahead of a `path:line:`
            // match is exactly the noise that defeats the search classifier —
            // and this face of the same classifier did not, so a colourised
            // result that was *under* budget at ingress (hygiene only runs over
            // budget) reached the stale pass with its escapes intact. `bash`
            // output arrives sanitized; MCP text results do not.
            let cleaned = sanitize_command_output(&original_text);
            // Stale results are sized aggressively, and `min_tokens_to_prune` is
            // the honest target: it is by definition the size below which a
            // result is no longer worth pruning. The alternative this competes
            // with is a one-line placeholder, so an over-tight reduction is
            // still far more signal than the fallback.
            let replacement =
                match structured::reduce_within(&cleaned, Some(self.min_tokens_to_prune)) {
                    Some(reduction) => {
                        let rendered = reduction.render();
                        if estimate_tokens_smart(&rendered) < original_tokens {
                            rendered
                        } else {
                            first_line_placeholder(tool_name, original_tokens, &cleaned)
                        }
                    }
                    None => first_line_placeholder(tool_name, original_tokens, &cleaned),
                };
            let new_tokens = estimate_tokens_smart(&replacement);
            if new_tokens >= original_tokens {
                continue;
            }
            // Rebuild the content as [reduced text, …original image blocks in
            // order]. Image lifecycle policy belongs solely to
            // `HistoricalImageStrippingStage` (which runs later and keeps the
            // newest image) — silently dropping images here would also free
            // the sensor's per-image token charge without counting it. Since
            // the images stay, `total_freed` correctly reflects only the text
            // shrink (`tool_result_info` never counted image blocks).
            if let UnifiedMessage::ToolResult { content, .. } = msg {
                let mut new_content = vec![ContentBlock::Text {
                    text: replacement,
                    cache_control: None,
                }];
                new_content.extend(
                    content
                        .iter()
                        .filter(|b| matches!(b, ContentBlock::Image { .. }))
                        .cloned(),
                );
                *content = new_content;
            }
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
        assert!(text.starts_with("[pruned tool_result"), "got: {text}");
    }

    #[test]
    fn result_hint_first_line_and_line_count() {
        assert_eq!(result_hint("single line"), "single line");
        assert_eq!(result_hint("first\nsecond\nthird"), "first (3 lines)");
        // Leading blank lines are skipped; the first real line is the hint.
        assert_eq!(
            result_hint("   \n  real first  \nx"),
            "real first (3 lines)"
        );
        assert_eq!(result_hint(""), "");
    }

    #[test]
    fn result_hint_truncates_long_first_line_char_safely() {
        // 200 CJK chars on one line — truncation must land on a char boundary.
        let long = "上".repeat(200);
        let hint = result_hint(&long);
        assert!(hint.ends_with('…'), "got: {hint}");
        assert_eq!(hint.chars().count(), MAX_HINT_CHARS + 1);
    }

    #[tokio::test]
    async fn structured_log_reduction_keeps_signal() {
        // Realistic command output: a test summary followed by hundreds of
        // detail lines. The content-type router recognizes a log and keeps the
        // summary signal while dropping the noise — strictly better than the
        // old first-line placeholder.
        let body = format!("PASS: 312 tests passed in 4.1s\n{}", "detail\n".repeat(400));
        let mut messages = vec![
            UnifiedMessage::tool_result("call-1", "bash", body, false),
            UnifiedMessage::user("recent"),
        ];
        let stage = ToolResultPruningStage::default();
        let freed = stage.prepare(&mut messages, &make_pressure(), 1).await;
        assert!(freed > 0, "a large multi-line log must be reduced");
        let (_name, text) = messages[0].tool_result_info().expect("still a ToolResult");
        assert!(
            text.starts_with("[compacted log:"),
            "structured log path should be taken; got: {text}"
        );
        assert!(
            text.contains("PASS: 312 tests passed"),
            "the summary signal must survive; got: {text}"
        );
        assert!(
            text.contains("lines omitted"),
            "dropped noise must be marked; got: {text}"
        );
    }

    #[tokio::test]
    async fn unstructured_result_uses_first_line_placeholder() {
        // A single huge line of opaque content has no structure to route on, so
        // the first-line placeholder fallback still applies.
        let mut messages = vec![
            UnifiedMessage::tool_result("call-1", "Read", "z".repeat(3000), false),
            UnifiedMessage::user("recent"),
        ];
        let stage = ToolResultPruningStage::default();
        let freed = stage.prepare(&mut messages, &make_pressure(), 1).await;
        assert!(freed > 0, "a large opaque result must be pruned");
        let (_name, text) = messages[0].tool_result_info().expect("still a ToolResult");
        assert!(
            text.starts_with("[pruned tool_result: Read"),
            "unstructured content falls back to first-line placeholder; got: {text}"
        );
    }

    #[tokio::test]
    async fn preserves_image_blocks_when_pruning() {
        // A tool result carrying text + an image (e.g. a screenshot tool):
        // pruning must shrink the text but leave the image for
        // HistoricalImageStrippingStage to police, and `freed` must reflect
        // the text shrink only (the image's sensor charge is not freed).
        let text = "x".repeat(2000);
        let mut messages = vec![
            UnifiedMessage::ToolResult {
                tool_call_id: "call-1".to_string(),
                tool_name: "screenshot".to_string(),
                content: vec![
                    ContentBlock::Text {
                        text: text.clone(),
                        cache_control: None,
                    },
                    ContentBlock::Image {
                        data: "iVBORw0KGgo=".repeat(50),
                        mime_type: "image/png".to_string(),
                    },
                ],
                is_error: false,
            },
            UnifiedMessage::user("recent"),
        ];
        let stage = ToolResultPruningStage::default();
        let freed = stage.prepare(&mut messages, &make_pressure(), 1).await;
        assert!(freed > 0, "the large text must still be pruned");
        assert!(
            freed <= estimate_tokens_smart(&text),
            "freed must reflect the text shrink only, got {freed}"
        );
        let UnifiedMessage::ToolResult { content, .. } = &messages[0] else {
            panic!("still a ToolResult");
        };
        assert!(
            matches!(&content[0], ContentBlock::Text { text, .. } if text.starts_with("[pruned tool_result")),
            "first block must be the reduced text"
        );
        assert!(
            content
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. })),
            "the image block must survive pruning"
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
    async fn skips_already_persisted_markers() {
        // Layer 2 (apply_result_budget) emits markers of this shape. The
        // pruning stage must treat them as already-compact and leave the
        // path intact so the LLM can still recover the full output via
        // a subsequent `read_file`.
        let marker = "[Full output persisted: /tmp/aleph/x.txt (12000 tokens, bash)]";
        let mut messages = vec![
            UnifiedMessage::tool_result("call-1", "bash", marker.to_string(), false),
            UnifiedMessage::user("recent"),
        ];
        let stage = ToolResultPruningStage::default();
        let freed = stage.prepare(&mut messages, &make_pressure(), 1).await;
        assert_eq!(freed, 0, "persisted markers must not be re-pruned");
        let (_name, text) = messages[0].tool_result_info().expect("still a ToolResult");
        assert_eq!(text, marker, "marker text must remain verbatim");
    }

    /// Regression: Layer 2 composes `body\nfooter`, so the recovery marker is
    /// not at byte 0 on the hygiene path. Pruning it would delete the reduced
    /// body *and* the only pointer to the offloaded original — the round's
    /// reversibility guarantee depends on this staying true.
    #[tokio::test]
    async fn a_marker_below_an_inlined_body_is_not_re_pruned() {
        let composed = format!(
            "[compacted log: kept 15/2017 lines]\n{}\n{}",
            "test result: FAILED. 2000 passed; 1 failed".repeat(20),
            "[Full output persisted: /tmp/aleph/x.txt (23730 tokens, bash)]"
        );
        let mut messages = vec![
            UnifiedMessage::tool_result("call-1", "bash", composed.clone(), false),
            UnifiedMessage::user("recent"),
        ];
        let stage = ToolResultPruningStage::default();
        let freed = stage.prepare(&mut messages, &make_pressure(), 1).await;
        assert_eq!(freed, 0, "a result carrying a marker must not be re-pruned");
        let (_n, after) = messages[0].tool_result_info().expect("still a ToolResult");
        assert_eq!(after, composed, "text must survive verbatim");
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
