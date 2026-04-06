//! ToolAwareChunker — semantic unit parsing and pair-aware chunking.
//!
//! Treats `tool_use` / `tool_result` pairs as atomic semantic units that must
//! never be split across compaction chunks.  A [`ToolRound`] always contains
//! the tool-calling assistant message, the matching `ToolResult`, and the
//! optional follow-up assistant text that references the result.

use crate::providers::message::UnifiedMessage;

// =============================================================================
// SemanticUnit
// =============================================================================

/// A single atomic semantic unit within a conversation.
///
/// The chunker never splits a unit — even a large `ToolRound` that exceeds the
/// chunk token limit is placed in its own chunk rather than being fragmented.
#[derive(Debug, Clone)]
pub enum SemanticUnit {
    /// A plain user message at the given message index.
    UserMessage { index: usize },

    /// A plain assistant text message (no tool calls) at the given message index.
    AssistantText { index: usize },

    /// A complete tool interaction round: the assistant tool-call message, its
    /// matching `ToolResult`, and an optional follow-up assistant message.
    ToolRound {
        /// Index of the assistant message containing the `ToolCall` block.
        tool_use_index: usize,
        /// Index of the `ToolResult` message that answers the call.
        tool_result_index: usize,
        /// Index of the assistant follow-up message after the result, if any.
        follow_up_index: Option<usize>,
        /// Name of the first tool called in the round (for logging / scoring).
        tool_name: String,
    },
}

impl SemanticUnit {
    /// Returns all message indices covered by this unit, in ascending order.
    pub fn message_indices(&self) -> Vec<usize> {
        match self {
            SemanticUnit::UserMessage { index } => vec![*index],
            SemanticUnit::AssistantText { index } => vec![*index],
            SemanticUnit::ToolRound {
                tool_use_index,
                tool_result_index,
                follow_up_index,
                ..
            } => {
                let mut indices = vec![*tool_use_index, *tool_result_index];
                if let Some(fu) = follow_up_index {
                    indices.push(*fu);
                }
                indices
            }
        }
    }

    /// Estimated token size of this unit across all its messages.
    ///
    /// Uses `content_len / ratio` as a proxy, matching the estimator used
    /// throughout the compaction pipeline.
    pub fn token_size(&self, messages: &[UnifiedMessage], ratio: f64) -> usize {
        self.message_indices()
            .iter()
            .filter_map(|&i| messages.get(i))
            .map(|msg| {
                let text = msg.text_content();
                (text.len() as f64 / ratio).ceil() as usize
            })
            .sum()
    }

    /// The earliest (smallest) message index in this unit.
    pub fn first_index(&self) -> usize {
        match self {
            SemanticUnit::UserMessage { index } => *index,
            SemanticUnit::AssistantText { index } => *index,
            SemanticUnit::ToolRound { tool_use_index, .. } => *tool_use_index,
        }
    }
}

// =============================================================================
// SemanticChunk
// =============================================================================

/// A group of [`SemanticUnit`]s that fit within a token budget.
#[derive(Debug, Clone)]
pub struct SemanticChunk {
    /// Ordered list of semantic units in this chunk.
    pub units: Vec<SemanticUnit>,
    /// Pre-computed total token estimate for all units.
    pub total_tokens: usize,
}

impl SemanticChunk {
    /// Flat-map all message indices across every unit in the chunk, in order.
    pub fn message_indices(&self) -> Vec<usize> {
        self.units
            .iter()
            .flat_map(|u| u.message_indices())
            .collect()
    }
}

// =============================================================================
// parse_semantic_units
// =============================================================================

/// Parse a message slice into a sequence of [`SemanticUnit`]s.
///
/// Rules (evaluated in order for each cursor position):
/// 1. An `Assistant` message with tool calls is the start of a `ToolRound`.
///    - The immediately following message must be a `ToolResult` to form a round.
///    - After the `ToolResult`, an optional plain `Assistant` message (no tool
///      calls) is absorbed as `follow_up`.
///    - If the next message is *not* a `ToolResult`, the assistant message is
///      treated as a plain `AssistantText`.
/// 2. A `User` message → `UserMessage`.
/// 3. A `ToolResult` without a preceding tool call (orphan) → `UserMessage` by
///    index (rare; the provider layer should already have repaired orphans).
/// 4. A plain `Assistant` message → `AssistantText`.
pub fn parse_semantic_units(messages: &[UnifiedMessage]) -> Vec<SemanticUnit> {
    let mut units = Vec::new();
    let mut i = 0;

    while i < messages.len() {
        let msg = &messages[i];

        if msg.has_tool_calls() {
            // Potential start of a ToolRound.
            let tool_use_index = i;
            let tool_name = extract_first_tool_name(msg);

            // The very next message must be a ToolResult.
            if i + 1 < messages.len() && messages[i + 1].is_tool_result() {
                let tool_result_index = i + 1;
                i += 2; // consume tool_use + tool_result

                // Optionally absorb a plain assistant follow-up.
                let follow_up_index = if i < messages.len()
                    && messages[i].is_assistant()
                    && !messages[i].has_tool_calls()
                {
                    let idx = i;
                    i += 1;
                    Some(idx)
                } else {
                    None
                };

                units.push(SemanticUnit::ToolRound {
                    tool_use_index,
                    tool_result_index,
                    follow_up_index,
                    tool_name,
                });
            } else {
                // No matching ToolResult — treat as plain assistant text.
                units.push(SemanticUnit::AssistantText {
                    index: tool_use_index,
                });
                i += 1;
            }
        } else if msg.is_user() {
            units.push(SemanticUnit::UserMessage { index: i });
            i += 1;
        } else if msg.is_tool_result() {
            // Orphan ToolResult — no preceding ToolCall.  Treat as UserMessage
            // by convention so downstream compactors still see the content.
            units.push(SemanticUnit::UserMessage { index: i });
            i += 1;
        } else {
            // Plain assistant message without tool calls.
            units.push(SemanticUnit::AssistantText { index: i });
            i += 1;
        }
    }

    units
}

/// Extract the name of the first tool call from an assistant message.
fn extract_first_tool_name(msg: &UnifiedMessage) -> String {
    msg.tool_calls()
        .into_iter()
        .next()
        .map(|(_, name, _)| name.to_owned())
        .unwrap_or_default()
}

// =============================================================================
// ToolAwareChunker
// =============================================================================

/// Greedy chunker that groups [`SemanticUnit`]s into [`SemanticChunk`]s while
/// respecting a per-chunk token limit and never splitting a [`SemanticUnit`].
pub struct ToolAwareChunker {
    /// Maximum tokens allowed in a single chunk.
    pub chunk_token_limit: usize,
    /// Characters-per-token ratio used for token estimation.
    pub token_ratio: f64,
}

impl ToolAwareChunker {
    /// Create a new chunker with the given token limit and estimation ratio.
    pub fn new(chunk_token_limit: usize, token_ratio: f64) -> Self {
        Self {
            chunk_token_limit,
            token_ratio,
        }
    }

    /// Chunk `units` into [`SemanticChunk`]s using a greedy algorithm.
    ///
    /// # Parameters
    /// - `units` — the semantic units to group.
    /// - `messages` — the original message slice (used for token estimation).
    /// - `fresh_start` — units whose `first_index() >= fresh_start` are excluded
    ///   (they belong to the protected "fresh tail").
    ///
    /// # Invariants
    /// - A single unit is **never split**, even if it alone exceeds the limit.
    /// - An oversized unit is placed in its own chunk.
    pub fn chunk(
        &self,
        units: &[SemanticUnit],
        messages: &[UnifiedMessage],
        fresh_start: usize,
    ) -> Vec<SemanticChunk> {
        let mut chunks: Vec<SemanticChunk> = Vec::new();
        let mut current_units: Vec<SemanticUnit> = Vec::new();
        let mut current_tokens: usize = 0;

        for unit in units {
            // Skip units that touch the fresh tail.
            if unit.first_index() >= fresh_start {
                break;
            }

            let unit_tokens = unit.token_size(messages, self.token_ratio);

            // Would adding this unit exceed the limit, and do we have content?
            if current_tokens + unit_tokens > self.chunk_token_limit && !current_units.is_empty() {
                // Flush the current chunk.
                chunks.push(SemanticChunk {
                    units: std::mem::take(&mut current_units),
                    total_tokens: current_tokens,
                });
                current_tokens = 0;
            }

            // Always add the unit (even if it alone exceeds the limit).
            current_tokens += unit_tokens;
            current_units.push(unit.clone());
        }

        // Flush any remaining units.
        if !current_units.is_empty() {
            chunks.push(SemanticChunk {
                units: current_units,
                total_tokens: current_tokens,
            });
        }

        chunks
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::{ContentBlock, UnifiedMessage};
    use serde_json::json;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn user_msg(text: &str) -> UnifiedMessage {
        UnifiedMessage::user(text)
    }

    fn assistant_msg(text: &str) -> UnifiedMessage {
        UnifiedMessage::assistant(text)
    }

    fn tool_use_msg(call_id: &str, tool_name: &str) -> UnifiedMessage {
        UnifiedMessage::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: call_id.to_owned(),
                name: tool_name.to_owned(),
                arguments: json!({}),
            }],
        }
    }

    fn tool_result_msg(call_id: &str, tool_name: &str, output: &str) -> UnifiedMessage {
        UnifiedMessage::tool_result(call_id, tool_name, output, false)
    }

    // ── tests ─────────────────────────────────────────────────────────────────

    #[test]
    fn parse_simple_conversation() {
        // 3 messages: user, assistant, user → 3 units
        let messages = vec![
            user_msg("Hello"),
            assistant_msg("Hi there!"),
            user_msg("How are you?"),
        ];

        let units = parse_semantic_units(&messages);
        assert_eq!(units.len(), 3);

        assert!(matches!(units[0], SemanticUnit::UserMessage { index: 0 }));
        assert!(matches!(units[1], SemanticUnit::AssistantText { index: 1 }));
        assert!(matches!(units[2], SemanticUnit::UserMessage { index: 2 }));
    }

    #[test]
    fn parse_tool_round_groups_correctly() {
        // 4 messages: user, tool_use, tool_result, assistant follow-up
        // Expected: 2 units — UserMessage + ToolRound(with follow_up)
        let messages = vec![
            user_msg("Search for cats"),
            tool_use_msg("c1", "web_search"),
            tool_result_msg("c1", "web_search", "Found 42 cat articles"),
            assistant_msg("Here are the results about cats!"),
        ];

        let units = parse_semantic_units(&messages);
        assert_eq!(units.len(), 2, "expected 2 units, got {units:?}");

        assert!(matches!(units[0], SemanticUnit::UserMessage { index: 0 }));

        match &units[1] {
            SemanticUnit::ToolRound {
                tool_use_index,
                tool_result_index,
                follow_up_index,
                tool_name,
            } => {
                assert_eq!(*tool_use_index, 1);
                assert_eq!(*tool_result_index, 2);
                assert_eq!(*follow_up_index, Some(3));
                assert_eq!(tool_name, "web_search");
            }
            other => panic!("expected ToolRound, got {other:?}"),
        }
    }

    #[test]
    fn chunking_respects_token_limit() {
        // 3 messages each with ~57 tokens of content (ratio = 4.0 → ~228 chars each)
        // With a 100-token limit, each message should land in its own chunk.
        let content = "x".repeat(230); // ~57 tokens at ratio 4.0
        let messages = vec![
            user_msg(&content),
            assistant_msg(&content),
            user_msg(&content),
        ];

        let units = parse_semantic_units(&messages);
        assert_eq!(units.len(), 3);

        let chunker = ToolAwareChunker::new(100, 4.0);
        let chunks = chunker.chunk(&units, &messages, messages.len());

        assert!(
            chunks.len() >= 2,
            "expected at least 2 chunks with 100-token limit, got {}",
            chunks.len()
        );
    }

    #[test]
    fn chunking_never_splits_tool_round() {
        // Build a ToolRound that is large enough to exceed even a tiny limit on its own.
        let large_output = "y".repeat(400); // ~100 tokens at ratio 4.0
        let messages = vec![
            user_msg("Do something"),
            tool_use_msg("c2", "big_tool"),
            tool_result_msg("c2", "big_tool", &large_output),
            assistant_msg("Done."),
        ];

        let units = parse_semantic_units(&messages);

        // Use a very small token limit (10) — the ToolRound alone exceeds it.
        let chunker = ToolAwareChunker::new(10, 4.0);
        let chunks = chunker.chunk(&units, &messages, messages.len());

        // Every chunk must contain at most one ToolRound, and all indices of a
        // ToolRound must be in the same chunk.
        for chunk in &chunks {
            for unit in &chunk.units {
                if let SemanticUnit::ToolRound {
                    tool_use_index,
                    tool_result_index,
                    follow_up_index,
                    ..
                } = unit
                {
                    let chunk_indices = chunk.message_indices();
                    assert!(
                        chunk_indices.contains(tool_use_index),
                        "tool_use_index missing from chunk"
                    );
                    assert!(
                        chunk_indices.contains(tool_result_index),
                        "tool_result_index missing from chunk"
                    );
                    if let Some(fu) = follow_up_index {
                        assert!(
                            chunk_indices.contains(fu),
                            "follow_up_index missing from chunk"
                        );
                    }
                }
            }
        }

        // Sanity: all 4 messages accounted for across all chunks.
        let all_indices: Vec<usize> = chunks.iter().flat_map(|c| c.message_indices()).collect();
        assert_eq!(all_indices.len(), 4);
    }
}
