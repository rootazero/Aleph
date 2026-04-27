//! LLM-based context compaction module.
//!
//! Replaces old conversation history with concise summaries via a side-channel
//! LLM call. Falls back to deterministic truncation when the LLM call fails.

use std::time::Duration;

use super::summary_utils::{strip_analysis_block, IDENTIFIER_PRESERVATION};
use crate::memory::session_compactor::summary_source::SessionSummarySource;
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

/// Strategy used during compaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactStrategy {
    /// Successfully summarized via a side-channel LLM call.
    LlmSummary,
    /// LLM call failed; fell back to keeping only the first line of each message.
    DeterministicTruncation,
    /// Reused existing session summaries — zero API cost.
    SessionMemoryReuse,
    /// Compaction was skipped entirely.
    Skipped { reason: String },
}

/// Result of a compaction attempt.
#[derive(Debug, Clone)]
pub struct CompactResult {
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub strategy_used: CompactStrategy,
}

/// Configuration for [`ContextCompactor`].
#[derive(Debug, Clone)]
pub struct CompactorConfig {
    /// Number of recent messages to keep untouched (default: 6).
    pub fresh_tail: usize,
    /// Target compression ratio (default: 0.25).
    pub target_ratio: f32,
    /// Maximum messages in the compression window (default: 40).
    pub max_window: usize,
    /// Timeout for the side-channel LLM call (default: 15 s).
    pub timeout: Duration,
    /// Whether to fall back to deterministic truncation on LLM failure (default: true).
    pub fallback_to_truncation: bool,
}

impl Default for CompactorConfig {
    fn default() -> Self {
        Self {
            fresh_tail: 6,
            target_ratio: 0.25,
            max_window: 40,
            timeout: Duration::from_secs(15),
            fallback_to_truncation: true,
        }
    }
}

/// LLM-based context compactor.
///
/// Compresses older conversation history into a concise summary, keeping
/// recent messages intact. Uses a side-channel LLM call for summarization
/// and falls back to deterministic truncation when the call fails.
pub struct ContextCompactor {
    provider: Arc<dyn AiProvider>,
    config: CompactorConfig,
}

impl ContextCompactor {
    /// Create a new compactor with the given provider and configuration.
    pub fn new(provider: Arc<dyn AiProvider>, config: CompactorConfig) -> Self {
        Self { provider, config }
    }

    /// Compact older messages in the conversation history.
    ///
    /// The `fresh_tail` parameter overrides `config.fresh_tail` when larger.
    ///
    /// # Flow
    ///
    /// 1. Determine the compression window (everything before the fresh tail).
    /// 2. Skip if the window is too small or already compacted.
    /// 3. Serialize the window into a transcript and estimate tokens.
    /// 4. Call the LLM with a summarization prompt (with timeout).
    /// 5. On success: replace the window with a single summary message.
    /// 6. On failure + fallback enabled: deterministic truncation.
    /// 7. On failure + fallback disabled: skip.
    pub async fn compact(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        fresh_tail: usize,
        summary_source: Option<&SessionSummarySource>,
    ) -> anyhow::Result<CompactResult> {
        let effective_tail = fresh_tail.max(self.config.fresh_tail);

        // Step 1: determine compression window
        if messages.len() <= effective_tail {
            return Ok(CompactResult {
                tokens_before: 0,
                tokens_after: 0,
                strategy_used: CompactStrategy::Skipped {
                    reason: "not enough messages to compact".into(),
                },
            });
        }

        let cut_end = messages.len() - effective_tail;
        if cut_end <= 1 {
            return Ok(CompactResult {
                tokens_before: 0,
                tokens_after: 0,
                strategy_used: CompactStrategy::Skipped {
                    reason: "compression window too small".into(),
                },
            });
        }

        // Step 2: idempotency check — skip if already compacted and window is small
        if let Some(first_text) = first_message_text(&messages[0]) {
            if first_text.starts_with("[Context Summary]") && cut_end <= 2 {
                return Ok(CompactResult {
                    tokens_before: 0,
                    tokens_after: 0,
                    strategy_used: CompactStrategy::Skipped {
                        reason: "already compacted with small window".into(),
                    },
                });
            }
        }

        // Step 3: limit window and serialize
        let window_start = cut_end.saturating_sub(self.config.max_window);

        // Fast path: try to reuse existing session summaries (zero API cost)
        if let Some(source) = summary_source {
            if let Some(reuse_result) = source.try_reuse(messages, window_start, cut_end).await {
                tracing::info!(
                    tokens_before = reuse_result.tokens_before,
                    tokens_after = reuse_result.tokens_after,
                    "Compaction via session memory reuse (zero API cost)"
                );
                return Ok(reuse_result);
            }
        }

        let window = &messages[window_start..cut_end];
        let transcript = serialize_transcript(window);
        let tokens_before = estimate_tokens(&transcript);

        // Step 4: build prompt with token budget
        let token_budget = (tokens_before as f32 * self.config.target_ratio) as usize;
        let prompt = format!(
            "Summarize the following conversation transcript in at most {token_budget} tokens.\n\
             \n\
             First, analyze the conversation in an <analysis> block (this will be stripped):\n\
             \n\
             <analysis>\n\
             1. User's primary request and intent\n\
             2. Key technical concepts and decisions made\n\
             3. Files and code sections involved (preserve exact paths)\n\
             4. Errors encountered and how they were resolved\n\
             5. Problem-solving approaches tried (what worked, what didn't)\n\
             </analysis>\n\
             \n\
             Then produce the final summary in a <summary> block using these MANDATORY sections:\n\
             \n\
             <summary>\n\
             ## Primary Request\n\
             [User's primary request and intent — never lose this]\n\
             \n\
             ## Key Decisions\n\
             [Decisions made and their rationale]\n\
             \n\
             ## Files & Code\n\
             [File paths and code sections involved — preserve exact paths]\n\
             \n\
             ## Current State\n\
             [Most recent operations and current work state, detailed]\n\
             \n\
             ## Pending\n\
             [Pending tasks, unresolved problems, and next steps]\n\
             </summary>\n\
             \n\
             Omit: greetings, filler, redundant confirmations.{IDENTIFIER_PRESERVATION}\n\
             \n\
             ---TRANSCRIPT---\n{transcript}\n---END---"
        );

        // Step 5–7: attempt LLM call with timeout
        let llm_result = tokio::time::timeout(self.config.timeout, self.call_llm(&prompt)).await;

        match llm_result {
            Ok(Ok(summary)) if !summary.trim().is_empty() => {
                // Success: strip analysis scratchpad, then drain old window and insert summary
                let summary = strip_analysis_block(&summary);
                let summary_msg = UnifiedMessage::user(format!("[Context Summary]\n{}", summary));
                let tokens_after = estimate_tokens(&summary);

                // Remove the compressed window and insert the summary at window_start
                messages.drain(window_start..cut_end);
                messages.insert(window_start, summary_msg);

                Ok(CompactResult {
                    tokens_before,
                    tokens_after,
                    strategy_used: CompactStrategy::LlmSummary,
                })
            }
            Ok(Ok(_)) | Ok(Err(_)) | Err(_) => {
                // LLM failed or returned empty — try fallback
                if self.config.fallback_to_truncation {
                    let truncated = deterministic_truncation(window);
                    let tokens_after = estimate_tokens(&truncated);
                    let summary_msg =
                        UnifiedMessage::user(format!("[Context Summary]\n{}", truncated));

                    messages.drain(window_start..cut_end);
                    messages.insert(window_start, summary_msg);

                    Ok(CompactResult {
                        tokens_before,
                        tokens_after,
                        strategy_used: CompactStrategy::DeterministicTruncation,
                    })
                } else {
                    Ok(CompactResult {
                        tokens_before,
                        tokens_after: tokens_before,
                        strategy_used: CompactStrategy::Skipped {
                            reason: "LLM call failed and fallback disabled".into(),
                        },
                    })
                }
            }
        }
    }

    /// Side-channel LLM call for summarization.
    async fn call_llm(&self, prompt: &str) -> anyhow::Result<String> {
        let msgs = [UnifiedMessage::user(prompt)];
        let system =
            "You are a precise conversation summarizer. Output the analysis block followed by the summary block. No other text.";
        let payload = RequestPayload::new(&msgs).with_system(Some(system));
        let response: ProviderResponse = self.provider.process(payload).await?;
        Ok(response.text.unwrap_or_default())
    }
}

// === Helper functions ===

/// Extract the text content of the first content block in a message (if Text).
fn first_message_text(msg: &UnifiedMessage) -> Option<&str> {
    msg.content_blocks().first().and_then(|b| b.as_text())
}

/// Serialize a slice of messages into a human-readable transcript.
fn serialize_transcript(messages: &[UnifiedMessage]) -> String {
    let mut lines = Vec::with_capacity(messages.len());
    for msg in messages {
        let role = match msg {
            UnifiedMessage::User { .. } => "user",
            UnifiedMessage::Assistant { .. } => "assistant",
            UnifiedMessage::ToolResult { tool_name, .. } => {
                lines.push(format!(
                    "tool_result({}): {}",
                    tool_name,
                    msg.text_content()
                ));
                continue;
            }
        };
        lines.push(format!("{}: {}", role, msg.text_content()));
    }
    lines.join("\n")
}

/// Estimate token count using the 3.5 chars/token heuristic.
fn estimate_tokens(text: &str) -> usize {
    let char_count = text.chars().count();
    (char_count as f64 / 3.5).ceil() as usize
}

/// Deterministic truncation: keep only the first line of each message.
fn deterministic_truncation(messages: &[UnifiedMessage]) -> String {
    let mut lines = Vec::with_capacity(messages.len());
    for msg in messages {
        let role = match msg {
            UnifiedMessage::User { .. } => "user",
            UnifiedMessage::Assistant { .. } => "assistant",
            UnifiedMessage::ToolResult { tool_name, .. } => {
                let text = msg.text_content();
                let first_line = text.lines().next().unwrap_or("");
                lines.push(format!("tool_result({}): {}", tool_name, first_line));
                continue;
            }
        };
        let text = msg.text_content();
        let first_line = text.lines().next().unwrap_or("");
        lines.push(format!("{}: {}", role, first_line));
    }
    lines.join("\n")
}

// =============================================================================
// CompactionStrategy impl
// =============================================================================

use super::{
    CompactionContext, CompactionResult, CompactionStrategy, PressureLevel, TokenEstimate,
};

impl CompactionStrategy for ContextCompactor {
    fn name(&self) -> &str {
        "llm_summary"
    }

    fn estimate_savings(&self, ctx: &CompactionContext) -> TokenEstimate {
        let compressible = ctx
            .pressure
            .used_tokens
            .saturating_sub(ctx.fresh_tail_count * 200);
        TokenEstimate {
            estimated_savings: (compressible as f64 * self.config.target_ratio as f64) as usize,
            confidence: 0.5,
        }
    }

    fn execute<'a>(
        &'a self,
        ctx: &'a mut CompactionContext,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<CompactionResult>> + Send + 'a>,
    > {
        Box::pin(async move {
            let before = ctx.pressure.ratio;
            let result = self
                .compact(&mut ctx.messages, ctx.fresh_tail_count, None)
                .await?;
            let freed = result.tokens_before.saturating_sub(result.tokens_after);
            ctx.pressure.used_tokens = ctx.pressure.used_tokens.saturating_sub(freed);
            ctx.pressure.ratio = if ctx.pressure.budget_tokens == 0 {
                1.0
            } else {
                ctx.pressure.used_tokens as f64 / ctx.pressure.budget_tokens as f64
            };
            Ok(CompactionResult {
                freed_tokens: freed,
                compacted_count: 1,
                strategy_name: self.name().to_string(),
                pressure_before: before,
                pressure_after: ctx.pressure.ratio,
            })
        })
    }

    fn is_applicable(&self, ctx: &CompactionContext) -> bool {
        ctx.pressure_level >= PressureLevel::High
    }
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::mock::MockProvider;
    use crate::providers::MockError;

    fn make_messages(count: usize) -> Vec<UnifiedMessage> {
        let mut msgs = Vec::with_capacity(count);
        for i in 0..count {
            if i % 2 == 0 {
                msgs.push(UnifiedMessage::user(format!("User message {}", i)));
            } else {
                msgs.push(UnifiedMessage::assistant(format!(
                    "Assistant response {}",
                    i
                )));
            }
        }
        msgs
    }

    #[tokio::test]
    async fn compacts_when_window_available() {
        let provider = Arc::new(MockProvider::new("Summary of earlier conversation."));
        let config = CompactorConfig::default();
        let compactor = ContextCompactor::new(provider, config);

        let mut messages = make_messages(12);
        let result = compactor.compact(&mut messages, 6, None).await.unwrap();

        assert_eq!(result.strategy_used, CompactStrategy::LlmSummary);
        assert!(result.tokens_after < result.tokens_before);
        // Original: 12 messages. Window = first 6 (indices 0..6).
        // After: 1 summary + 6 fresh = 7 messages.
        assert_eq!(messages.len(), 7);

        // First message should be the summary
        let first_text = first_message_text(&messages[0]).unwrap();
        assert!(first_text.starts_with("[Context Summary]"));
    }

    #[tokio::test]
    async fn skips_when_window_too_small() {
        let provider = Arc::new(MockProvider::new("ignored"));
        let config = CompactorConfig::default();
        let compactor = ContextCompactor::new(provider, config);

        let mut messages = make_messages(4);
        let original_len = messages.len();
        let result = compactor.compact(&mut messages, 6, None).await.unwrap();

        assert!(matches!(
            result.strategy_used,
            CompactStrategy::Skipped { .. }
        ));
        assert_eq!(messages.len(), original_len);
    }

    #[tokio::test]
    async fn falls_back_to_truncation_on_provider_failure() {
        let provider =
            Arc::new(MockProvider::new("ignored").with_error(MockError::Provider("fail".into())));
        let config = CompactorConfig {
            fallback_to_truncation: true,
            ..Default::default()
        };
        let compactor = ContextCompactor::new(provider, config);

        let mut messages = make_messages(12);
        let result = compactor.compact(&mut messages, 6, None).await.unwrap();

        assert_eq!(
            result.strategy_used,
            CompactStrategy::DeterministicTruncation
        );
        // 1 summary + 6 fresh = 7
        assert_eq!(messages.len(), 7);

        let first_text = first_message_text(&messages[0]).unwrap();
        assert!(first_text.starts_with("[Context Summary]"));
    }

    #[tokio::test]
    async fn idempotent_on_already_compacted() {
        let provider = Arc::new(MockProvider::new("ignored"));
        let config = CompactorConfig::default();
        let compactor = ContextCompactor::new(provider, config);

        // Simulate already-compacted state: summary + a few messages
        let mut messages = vec![
            UnifiedMessage::user("[Context Summary]\nPrevious conversation summary."),
            UnifiedMessage::assistant("Continuing from summary."),
        ];
        // Add fresh tail
        for i in 0..6 {
            messages.push(UnifiedMessage::user(format!("Fresh message {}", i)));
        }
        // Total: 8 messages. cut_end = 8 - 6 = 2. First starts with [Context Summary] and cut_end <= 2.
        let original_len = messages.len();
        let result = compactor.compact(&mut messages, 6, None).await.unwrap();

        assert!(matches!(
            result.strategy_used,
            CompactStrategy::Skipped { reason } if reason.contains("already compacted")
        ));
        assert_eq!(messages.len(), original_len);
    }
}
