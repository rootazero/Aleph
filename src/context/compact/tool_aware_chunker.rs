//! Semantic-unit parsing and greedy chunking for post-turn compaction.
//!
//! The sole production caller (`session_compactor::post_turn_compress`)
//! flattens history into plain user/assistant text messages before parsing,
//! so every message maps to exactly one atomic [`SemanticUnit`].  The former
//! tool-round pairing machinery (tool_use/tool_result grouping, follow-up
//! absorption, orphan repair, fresh-tail straddle guard) was unreachable from
//! production and has been removed; the `ToolAware` name is retained for API
//! stability.

use crate::providers::message::UnifiedMessage;

// =============================================================================
// SemanticUnit
// =============================================================================

/// A single atomic semantic unit within a conversation.
///
/// The chunker never splits a unit — even a message that alone exceeds the
/// chunk token limit is placed in its own chunk rather than being fragmented.
#[derive(Debug, Clone)]
pub enum SemanticUnit {
    /// A plain user message at the given message index.
    UserMessage { index: usize },

    /// A plain assistant text message at the given message index.
    AssistantText { index: usize },
}

impl SemanticUnit {
    /// Returns all message indices covered by this unit, in ascending order.
    #[must_use]
    pub fn message_indices(&self) -> Vec<usize> {
        match self {
            Self::UserMessage { index } | Self::AssistantText { index } => vec![*index],
        }
    }

    /// Estimated token size of this unit.
    ///
    /// Uses `content_len / ratio` as a proxy, matching the estimator used
    /// throughout the compaction pipeline.
    #[must_use]
    pub fn token_size(&self, messages: &[UnifiedMessage], ratio: f64) -> usize {
        self.message_indices()
            .iter()
            .filter_map(|&i| messages.get(i))
            .map(|msg| {
                let text = msg.text_content();
                (text.chars().count() as f64 / ratio).ceil() as usize
            })
            .sum()
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
    #[must_use]
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
/// Each message becomes exactly one unit: a `User` message → `UserMessage`,
/// anything else → `AssistantText`.
#[must_use]
pub fn parse_semantic_units(messages: &[UnifiedMessage]) -> Vec<SemanticUnit> {
    messages
        .iter()
        .enumerate()
        .map(|(index, msg)| {
            if msg.is_user() {
                SemanticUnit::UserMessage { index }
            } else {
                SemanticUnit::AssistantText { index }
            }
        })
        .collect()
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
    ///
    /// `ratio` must be `> 0.0`. A non-positive value would either yield
    /// `usize::MAX` (division by zero) or saturate the unsigned conversion
    /// (`< 0.0` yields 0), either of which silently disables the chunker's
    /// token-limit logic.
    pub token_ratio: f64,
}

impl ToolAwareChunker {
    /// Create a new chunker with the given token limit and estimation ratio.
    ///
    /// Falls back to the conservative default of `0.25` (chars-per-token
    /// heuristic) when `token_ratio <= 0.0` or is non-finite. The previous
    /// `assert!` would panic the agent loop on a misconfigured caller
    /// (e.g. a future MCP / Skill injecting a ratio override) — degrade
    /// gracefully instead of crashing the run.
    #[must_use]
    pub fn new(chunk_token_limit: usize, token_ratio: f64) -> Self {
        const DEFAULT_RATIO: f64 = 0.25;
        let ratio = if token_ratio.is_finite() && token_ratio > 0.0 {
            token_ratio
        } else {
            tracing::warn!(
                provided = token_ratio,
                "ToolAwareChunker: token_ratio out of range; falling back to default 0.25"
            );
            DEFAULT_RATIO
        };
        Self {
            chunk_token_limit,
            token_ratio: ratio,
        }
    }

    /// Chunk `units` into [`SemanticChunk`]s using a greedy algorithm.
    ///
    /// # Invariants
    /// - A single unit is **never split**, even if it alone exceeds the limit.
    /// - An oversized unit is placed in its own chunk.
    #[must_use]
    pub fn chunk(&self, units: &[SemanticUnit], messages: &[UnifiedMessage]) -> Vec<SemanticChunk> {
        let mut chunks: Vec<SemanticChunk> = Vec::new();
        let mut current_units: Vec<SemanticUnit> = Vec::new();
        let mut current_tokens: usize = 0;

        for unit in units {
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

    // ── helpers ──────────────────────────────────────────────────────────────

    fn user_msg(text: &str) -> UnifiedMessage {
        UnifiedMessage::user(text)
    }

    fn assistant_msg(text: &str) -> UnifiedMessage {
        UnifiedMessage::assistant(text)
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
        let chunks = chunker.chunk(&units, &messages);

        assert!(
            chunks.len() >= 2,
            "expected at least 2 chunks with 100-token limit, got {}",
            chunks.len()
        );
    }
}
