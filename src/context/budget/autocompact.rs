//! AutocompactStage — LLM-based conversation summarization.
//!
//! Calls an injected summarizer function to compress old conversation segments,
//! freeing tokens for new messages. Uses dependency injection so tests don't
//! need a real LLM.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use super::preflight::PreflightStage;
use super::pressure::estimate_tokens_smart;
use super::ContextPressure;
use crate::memory::session_compactor::context_window::partition_fresh_tail;
use crate::providers::message::UnifiedMessage;

/// Pressure ratio below which autocompact is skipped entirely.
const ACTIVATION_THRESHOLD: f64 = 0.65;

/// Minimum messages in the compressible zone before we attempt summarization.
const MIN_MESSAGES_TO_SUMMARIZE: usize = 6;

/// Default number of turns to wait between compaction attempts.
const DEFAULT_COOLDOWN_TURNS: usize = 10;

/// Maximum characters per message included in the summarization prompt.
const MAX_CHARS_PER_MSG: usize = 500;

type SummarizerFn =
    dyn Fn(String) -> Pin<Box<dyn Future<Output = anyhow::Result<String>> + Send>> + Send + Sync;

/// Async preflight stage that calls an LLM to summarize old conversation segments.
pub struct AutocompactStage {
    summarizer: Box<SummarizerFn>,
    cooldown_turns: usize,
    last_compact_turn: AtomicUsize,
    invocation_count: AtomicUsize,
}

impl AutocompactStage {
    /// Create with a custom summarizer (for testing and DI).
    pub fn new_with_summarizer<F, Fut>(f: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<String>> + Send + 'static,
    {
        Self {
            summarizer: Box::new(move |input| Box::pin(f(input))),
            cooldown_turns: DEFAULT_COOLDOWN_TURNS,
            last_compact_turn: AtomicUsize::new(0),
            invocation_count: AtomicUsize::new(0),
        }
    }

    /// No-op autocompact (when no LLM available).
    pub fn noop() -> Self {
        Self::new_with_summarizer(|_| async { Err(anyhow::anyhow!("no summarizer configured")) })
    }
}

#[async_trait]
impl PreflightStage for AutocompactStage {
    fn name(&self) -> &'static str {
        "autocompact"
    }

    async fn prepare(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        pressure: &ContextPressure,
        fresh_tail_count: usize,
    ) -> usize {
        // Gate 1: Skip if pressure is below activation threshold.
        if pressure.ratio < ACTIVATION_THRESHOLD {
            return 0;
        }

        // Gate 2: Skip if within cooldown period (monotonic counter, not messages.len()).
        let current_invocation = self.invocation_count.fetch_add(1, Ordering::Release);
        let last = self.last_compact_turn.load(Ordering::Acquire);
        if last > 0 && current_invocation.saturating_sub(last) < self.cooldown_turns {
            return 0;
        }

        // Determine the compressible zone: everything before the fresh tail.
        let boundary = partition_fresh_tail(messages, fresh_tail_count);
        if boundary == 0 {
            return 0;
        }

        // Preserve first user message (index 0) — summary starts at index 1.
        let summary_start = 1;
        if boundary <= summary_start {
            return 0;
        }

        // Gate 3: Skip if too few messages in compressible zone.
        let compressible_count = boundary - summary_start;
        if compressible_count < MIN_MESSAGES_TO_SUMMARIZE {
            return 0;
        }

        // Find safe boundary that doesn't orphan tool_results.
        let summary_end = find_safe_boundary(messages, boundary);
        if summary_end <= summary_start {
            return 0;
        }

        // Estimate tokens before compaction.
        let tokens_before: usize = messages[summary_start..summary_end]
            .iter()
            .map(|m| estimate_tokens_smart(&m.text_content()))
            .sum();

        // Build conversation text for summarization prompt.
        let mut prompt_parts = Vec::new();
        for msg in &messages[summary_start..summary_end] {
            let role = if msg.is_user() {
                "user"
            } else if msg.is_assistant() {
                "assistant"
            } else {
                "tool"
            };
            let text = truncate_for_summary(&msg.text_content());
            prompt_parts.push(format!("[{role}]: {text}"));
        }
        let prompt = format!(
            "Summarize the following conversation segment concisely, preserving key decisions, code references, and action items:\n\n{}",
            prompt_parts.join("\n")
        );

        // Call the summarizer — on error, log warning and return 0.
        let summary = match (self.summarizer)(prompt).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    target: "autocompact",
                    error = %e,
                    "Summarizer failed — skipping autocompact"
                );
                return 0;
            }
        };

        // Build the compact message.
        let compact_msg = UnifiedMessage::user(format!(
            "[Conversation summary — earlier messages were compressed to save context]\n\n{summary}"
        ));

        // Estimate tokens after compaction.
        let tokens_after = estimate_tokens_smart(&compact_msg.text_content());

        // Replace the summary range with the compact message.
        messages.splice(summary_start..summary_end, std::iter::once(compact_msg));

        // Update cooldown tracking (monotonic invocation count).
        self.last_compact_turn
            .store(current_invocation + 1, Ordering::Release);

        // Return tokens freed.
        tokens_before.saturating_sub(tokens_after)
    }
}

/// Walk backwards from `target` to avoid orphaning tool_result messages.
///
/// A tool_result should always follow its corresponding assistant tool_call,
/// so we avoid splitting right before a tool_result.
fn find_safe_boundary(messages: &[UnifiedMessage], target: usize) -> usize {
    let mut pos = target;
    while pos > 0 && messages[pos.saturating_sub(1)].is_tool_result() {
        pos -= 1;
    }
    pos
}

/// Truncate a message's text content for inclusion in the summarization prompt.
fn truncate_for_summary(text: &str) -> String {
    if text.chars().count() <= MAX_CHARS_PER_MSG {
        text.to_string()
    } else {
        let end = text
            .char_indices()
            .nth(MAX_CHARS_PER_MSG)
            .map(|(i, _)| i)
            .unwrap_or(text.len());
        format!("{}... [truncated]", &text[..end])
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn critical_pressure() -> ContextPressure {
        ContextPressure {
            used_tokens: 900,
            budget_tokens: 1000,
            ratio: 0.9,
            overhead_tokens: 100,
            available_for_messages: 900,
        }
    }

    fn low_pressure() -> ContextPressure {
        ContextPressure {
            used_tokens: 300,
            budget_tokens: 1000,
            ratio: 0.3,
            overhead_tokens: 100,
            available_for_messages: 900,
        }
    }

    #[tokio::test]
    async fn skips_at_low_pressure() {
        let stage = AutocompactStage::new_with_summarizer(|_| async {
            Ok("should not be called".to_string())
        });
        let mut msgs = vec![
            UnifiedMessage::user("hello"),
            UnifiedMessage::assistant("hi"),
        ];
        let freed = stage.prepare(&mut msgs, &low_pressure(), 2).await;
        assert_eq!(freed, 0);
        assert_eq!(msgs.len(), 2, "messages should be unchanged");
    }

    #[tokio::test]
    async fn skips_when_too_few_messages() {
        let stage = AutocompactStage::new_with_summarizer(|_| async {
            Ok("should not be called".to_string())
        });
        let mut msgs = vec![
            UnifiedMessage::user("hello"),
            UnifiedMessage::assistant("hi"),
        ];
        let freed = stage.prepare(&mut msgs, &critical_pressure(), 1).await;
        assert_eq!(freed, 0);
        assert_eq!(msgs.len(), 2, "messages should be unchanged");
    }

    #[tokio::test]
    async fn compacts_old_messages_preserving_first_and_tail() {
        let stage = AutocompactStage::new_with_summarizer(|_| async {
            Ok("Summary: user asked to refactor files A, B, C.".to_string())
        });

        let mut msgs = vec![
            UnifiedMessage::user("Please help me refactor"), // index 0: preserved
            UnifiedMessage::assistant("Sure"),               // index 1: compressible
            UnifiedMessage::user("file A"),                  // index 2
            UnifiedMessage::tool_result("c1", "Read", &"x".repeat(2000), false), // index 3
            UnifiedMessage::assistant("I see A"),            // index 4
            UnifiedMessage::user("file B"),                  // index 5
            UnifiedMessage::tool_result("c2", "Read", &"y".repeat(2000), false), // index 6
            UnifiedMessage::assistant("I see B"),            // index 7
            UnifiedMessage::user("file C"),                  // index 8
            UnifiedMessage::tool_result("c3", "Read", &"z".repeat(2000), false), // index 9
            UnifiedMessage::assistant("I see C"),            // index 10
            UnifiedMessage::user("Now refactor"),            // index 11: fresh tail
            UnifiedMessage::assistant("Starting"),           // index 12: fresh tail
        ];
        let original_len = msgs.len();

        let freed = stage.prepare(&mut msgs, &critical_pressure(), 2).await;

        // Should have freed tokens.
        assert!(freed > 0, "should have freed tokens, got {freed}");

        // First message should still be preserved.
        assert!(msgs[0].is_user());
        assert!(
            msgs[0].text_content().contains("Please help me refactor"),
            "first user message must be preserved"
        );

        // Last two messages (fresh tail) should be preserved.
        let tail = &msgs[msgs.len() - 2..];
        assert!(
            tail[0].text_content().contains("Now refactor"),
            "fresh tail user message must be preserved"
        );
        assert!(
            tail[1].text_content().contains("Starting"),
            "fresh tail assistant message must be preserved"
        );

        // A summary message should exist.
        let has_summary = msgs
            .iter()
            .any(|m| m.text_content().contains("[Conversation summary"));
        assert!(has_summary, "should contain a summary message");

        // Total messages should be fewer than original.
        assert!(
            msgs.len() < original_len,
            "message count should decrease: {} < {}",
            msgs.len(),
            original_len
        );
    }

    #[tokio::test]
    async fn graceful_on_summarizer_failure() {
        let stage = AutocompactStage::new_with_summarizer(|_| async {
            Err(anyhow::anyhow!("LLM unavailable"))
        });

        let mut msgs = vec![
            UnifiedMessage::user("Please help me refactor"),
            UnifiedMessage::assistant("Sure"),
            UnifiedMessage::user("file A"),
            UnifiedMessage::tool_result("c1", "Read", &"x".repeat(2000), false),
            UnifiedMessage::assistant("I see A"),
            UnifiedMessage::user("file B"),
            UnifiedMessage::tool_result("c2", "Read", &"y".repeat(2000), false),
            UnifiedMessage::assistant("I see B"),
            UnifiedMessage::user("file C"),
            UnifiedMessage::tool_result("c3", "Read", &"z".repeat(2000), false),
            UnifiedMessage::assistant("I see C"),
            UnifiedMessage::user("Now refactor"),
            UnifiedMessage::assistant("Starting"),
        ];
        let original_len = msgs.len();

        let freed = stage.prepare(&mut msgs, &critical_pressure(), 2).await;

        assert_eq!(freed, 0, "should return 0 on summarizer failure");
        assert_eq!(
            msgs.len(),
            original_len,
            "messages should be unchanged on failure"
        );
    }
}
