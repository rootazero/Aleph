//! Hierarchical summarization engine.
//!
//! Builds a multi-depth summary tree over older conversation turns,
//! progressively condensing them while preserving the fresh tail verbatim.

use super::context_window::estimate_tokens;
use super::fallback::{target_tokens, FallbackLevel};
use crate::agent_loop::compaction::summary_utils::IDENTIFIER_PRESERVATION;
use crate::memory::{FactSource, NoteType, MemoryFact, MemoryLayer, MemoryScope, MemoryTier};

// Re-export for backwards compatibility.
pub use crate::agent_loop::compaction::summary_utils::strip_analysis_block;

// ---------------------------------------------------------------------------
// Depth-aware prompt templates
// ---------------------------------------------------------------------------

const LEAF_PROMPT: &str = "\
You are a conversation compressor. Condense the following conversation into a structured summary.\n\
\n\
First, analyze the conversation in an <analysis> block (this will be stripped before the summary enters context):\n\
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
Omit: greetings, filler, repeated information, verbose tool outputs already summarized.";

const D1_PROMPT: &str = "\
Condense these summaries into a higher-level session summary. Preserve:\n\
- Decisions and their rationale (especially decisions that supersede earlier ones)\n\
- Current task status (completed, in-progress, blocked)\n\
- Unresolved problems and blockers\n\
- Key files and components affected\n\
\n\
Omit: operational details of individual steps, file contents, error messages (unless unresolved).";

const D2_PROMPT: &str = "\
Create a milestone summary from these session summaries. Preserve:\n\
- Completed work and outcomes\n\
- Active constraints and decisions still in effect\n\
- Evolution of approach (what changed and why)\n\
- Remaining work items\n\
\n\
Omit: individual operation details, file-level changes, resolved errors.";

/// Select the correct prompt template for a given depth.
fn depth_prompt(depth: u32) -> &'static str {
    match depth {
        0 => LEAF_PROMPT,
        1 => D1_PROMPT,
        _ => D2_PROMPT,
    }
}

// ---------------------------------------------------------------------------
// build_summary_prompt
// ---------------------------------------------------------------------------

/// Build the full prompt string to send to the LLM for summarization.
///
/// Assembles: depth instruction + target tokens + previous context (if any)
/// + conversation + "Expand for details" instruction.
pub fn build_summary_prompt(
    messages: &[(String, String)], // (role, content)
    depth: u32,
    previous_context: Option<&str>,
    level: FallbackLevel,
) -> String {
    let instruction = depth_prompt(depth);

    // Estimate total input tokens so we can tell the LLM the target length.
    let input_tokens: usize = messages
        .iter()
        .map(|(_, content)| estimate_tokens(content, 3.5))
        .sum();
    let target = target_tokens(input_tokens, level);

    let mut prompt = String::new();

    // System-level instruction and target length hint.
    prompt.push_str(instruction);
    prompt.push_str(IDENTIFIER_PRESERVATION);
    prompt.push_str(&format!("\n\nTarget summary length: ~{} tokens.", target));

    // Inject previous context as a reminder if available.
    if let Some(ctx) = previous_context {
        if !ctx.is_empty() {
            prompt.push_str("\n\n--- Previous context ---\n");
            prompt.push_str(ctx);
            prompt.push_str("\n--- End previous context ---");
        }
    }

    // Append the conversation turns.
    prompt.push_str("\n\n--- Conversation ---");
    for (role, content) in messages {
        prompt.push('\n');
        prompt.push_str(&format!("[{}]: {}", role, content));
    }
    prompt.push_str("\n--- End conversation ---");

    // Final instruction to the model.
    prompt.push_str("\n\nExpand for details only when explicitly asked. Produce the summary now.");

    prompt
}

// ---------------------------------------------------------------------------
// chunk_messages
// ---------------------------------------------------------------------------

/// Group messages into chunks of approximately `chunk_tokens` tokens each.
///
/// `ratio` is the chars-per-token estimate (e.g. 3.5).
/// Each returned chunk contains at least one message.
pub fn chunk_messages(
    messages: &[(String, String)],
    chunk_tokens: usize,
    ratio: f64,
) -> Vec<Vec<(String, String)>> {
    if messages.is_empty() {
        return Vec::new();
    }

    let mut chunks: Vec<Vec<(String, String)>> = Vec::new();
    let mut current_chunk: Vec<(String, String)> = Vec::new();
    let mut current_tokens: usize = 0;

    for msg in messages {
        let msg_tokens = estimate_tokens(&msg.1, ratio);

        // If adding this message would overflow the chunk and the current chunk
        // is non-empty, flush it first.
        if current_tokens + msg_tokens > chunk_tokens && !current_chunk.is_empty() {
            chunks.push(std::mem::take(&mut current_chunk));
            current_tokens = 0;
        }

        current_chunk.push(msg.clone());
        current_tokens += msg_tokens;
    }

    // Flush the last partial chunk.
    if !current_chunk.is_empty() {
        chunks.push(current_chunk);
    }

    chunks
}

// ---------------------------------------------------------------------------
// summary_to_fact
// ---------------------------------------------------------------------------

/// Convert a summary text produced by the LLM into a [`MemoryFact`].
///
/// Layer assignment:
/// - d0 (leaf) → `L2Detail`
/// - d1        → `L1Overview`
/// - d2+       → `L0Abstract`
pub fn summary_to_fact(
    session_id: &str,
    depth: u32,
    seq: u32,
    summary_text: String,
    _source_message_count: usize,
    _source_token_count: usize,
    agent_id: &str,
) -> MemoryFact {
    let layer = match depth {
        0 => MemoryLayer::L2Detail,
        1 => MemoryLayer::L1Overview,
        _ => MemoryLayer::L0Abstract,
    };

    let path = format!("aleph://session/{session_id}/d{depth}/{seq}");

    MemoryFact::new(summary_text, NoteType::Other, Vec::new())
        .with_fact_source(FactSource::SessionCompressed)
        .with_scope(MemoryScope::SessionLocal)
        .with_tier(MemoryTier::ShortTerm)
        .with_layer(layer)
        .with_path(path)
        .with_confidence(0.9)
        .with_agent(agent_id.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: build a simple message list.
    fn msgs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(r, c)| (r.to_string(), c.to_string()))
            .collect()
    }

    // ------------------------------------------------------------------
    // build_summary_prompt — depth selection
    // ------------------------------------------------------------------

    #[test]
    fn test_build_prompt_leaf_contains_leaf_instruction() {
        let messages = msgs(&[("user", "Hello"), ("assistant", "Hi there")]);
        let prompt = build_summary_prompt(&messages, 0, None, FallbackLevel::Normal);
        assert!(
            prompt.contains("## Files & Code"),
            "leaf prompt should mention Files & Code section"
        );
        assert!(
            !prompt.contains("milestone summary"),
            "leaf prompt should not mention milestone summary"
        );
    }

    #[test]
    fn test_build_prompt_d1_contains_d1_instruction() {
        let messages = msgs(&[("assistant", "Summary of chunk 1")]);
        let prompt = build_summary_prompt(&messages, 1, None, FallbackLevel::Normal);
        assert!(
            prompt.contains("higher-level session summary"),
            "d1 prompt should mention higher-level session summary"
        );
    }

    #[test]
    fn test_build_prompt_d2_contains_d2_instruction() {
        let messages = msgs(&[("assistant", "Summary of session 1")]);
        let prompt = build_summary_prompt(&messages, 2, None, FallbackLevel::Normal);
        assert!(
            prompt.contains("milestone summary"),
            "d2+ prompt should mention milestone summary"
        );
    }

    #[test]
    fn test_build_prompt_depth_beyond_2_uses_d2_template() {
        let messages = msgs(&[("assistant", "Some text")]);
        let p2 = build_summary_prompt(&messages, 2, None, FallbackLevel::Normal);
        let p5 = build_summary_prompt(&messages, 5, None, FallbackLevel::Normal);
        // Both should contain the same depth-2 instruction text.
        assert_eq!(p2, p5);
    }

    // ------------------------------------------------------------------
    // build_summary_prompt — previous_context injection
    // ------------------------------------------------------------------

    #[test]
    fn test_build_prompt_injects_previous_context() {
        let messages = msgs(&[("user", "What's next?")]);
        let ctx = "We decided to use Rust for the backend.";
        let prompt = build_summary_prompt(&messages, 0, Some(ctx), FallbackLevel::Normal);
        assert!(
            prompt.contains(ctx),
            "prompt should include the previous context verbatim"
        );
        assert!(
            prompt.contains("Previous context"),
            "prompt should label the previous context section"
        );
    }

    #[test]
    fn test_build_prompt_no_previous_context_section_when_none() {
        let messages = msgs(&[("user", "Hello")]);
        let prompt = build_summary_prompt(&messages, 0, None, FallbackLevel::Normal);
        assert!(
            !prompt.contains("Previous context"),
            "prompt should not contain previous context section when None"
        );
    }

    #[test]
    fn test_build_prompt_empty_previous_context_omitted() {
        let messages = msgs(&[("user", "Hello")]);
        let prompt = build_summary_prompt(&messages, 0, Some(""), FallbackLevel::Normal);
        assert!(
            !prompt.contains("Previous context"),
            "prompt should not contain previous context section for empty string"
        );
    }

    // ------------------------------------------------------------------
    // chunk_messages
    // ------------------------------------------------------------------

    #[test]
    fn test_chunk_messages_empty() {
        let chunks = chunk_messages(&[], 100, 3.5);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_messages_all_fit_in_one() {
        // Each message is "hi" (~0-1 tokens), chunk budget is very large.
        let messages = msgs(&[("user", "hi"), ("assistant", "hello"), ("user", "bye")]);
        let chunks = chunk_messages(&messages, 10_000, 3.5);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 3);
    }

    #[test]
    fn test_chunk_messages_splits_correctly() {
        // Each message content is exactly 35 chars → ~10 tokens at ratio 3.5.
        // chunk_tokens = 15, so two messages (20 tokens) should split into two chunks.
        let content = "a".repeat(35); // 35 chars / 3.5 = 10 tokens
        let messages: Vec<(String, String)> = (0..4)
            .map(|_| ("user".to_string(), content.clone()))
            .collect();
        let chunks = chunk_messages(&messages, 15, 3.5);
        // 4 messages × 10 tokens each, budget 15 → first chunk gets 1 msg,
        // then the second fits 1, third fits 1, fourth fits 1 → 4 chunks.
        // Actually: first msg (10 tokens) < 15, add it. Second msg would make
        // 20 > 15, flush. So chunk [msg0], then [msg1], [msg2], [msg3].
        assert_eq!(chunks.len(), 4);
        for chunk in &chunks {
            assert_eq!(chunk.len(), 1);
        }
    }

    #[test]
    fn test_chunk_messages_single_large_message_forms_own_chunk() {
        // A single message larger than chunk_tokens still forms its own chunk.
        let big = "x".repeat(1000);
        let messages: Vec<(String, String)> = vec![
            ("user".to_string(), big.clone()),
            ("user".to_string(), "short".to_string()),
        ];
        let chunks = chunk_messages(&messages, 10, 3.5);
        // Big message alone, then short message alone.
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0][0].1, big);
    }

    // ------------------------------------------------------------------
    // summary_to_fact — field verification
    // ------------------------------------------------------------------

    #[test]
    fn test_summary_to_fact_fields() {
        let fact = summary_to_fact(
            "sess-123",
            0,
            1,
            "Summary text".to_string(),
            10,
            500,
            "agent-1",
        );
        assert_eq!(fact.fact_source, FactSource::SessionCompressed);
        assert_eq!(fact.scope, MemoryScope::SessionLocal);
        assert_eq!(fact.tier, MemoryTier::ShortTerm);
        assert_eq!(fact.layer, MemoryLayer::L2Detail);
        assert_eq!(fact.path, "aleph://session/sess-123/d0/1");
        assert!((fact.confidence - 0.9).abs() < 0.001);
        assert_eq!(fact.agent, "agent-1");
        assert_eq!(fact.content, "Summary text");
    }

    // ------------------------------------------------------------------
    // summary_to_fact — depth → layer mapping
    // ------------------------------------------------------------------

    #[test]
    fn test_summary_to_fact_depth0_is_l2_detail() {
        let fact = summary_to_fact("s", 0, 0, "x".into(), 0, 0, "a");
        assert_eq!(fact.layer, MemoryLayer::L2Detail);
    }

    #[test]
    fn test_summary_to_fact_depth1_is_l1_overview() {
        let fact = summary_to_fact("s", 1, 0, "x".into(), 0, 0, "a");
        assert_eq!(fact.layer, MemoryLayer::L1Overview);
    }

    #[test]
    fn test_summary_to_fact_depth2_is_l0_abstract() {
        let fact = summary_to_fact("s", 2, 0, "x".into(), 0, 0, "a");
        assert_eq!(fact.layer, MemoryLayer::L0Abstract);
    }

    #[test]
    fn test_summary_to_fact_depth_gt2_is_l0_abstract() {
        let fact = summary_to_fact("s", 5, 0, "x".into(), 0, 0, "a");
        assert_eq!(fact.layer, MemoryLayer::L0Abstract);
    }

    #[test]
    fn test_summary_to_fact_path_includes_depth_and_seq() {
        let fact = summary_to_fact("my-session", 1, 7, "x".into(), 0, 0, "a");
        assert_eq!(fact.path, "aleph://session/my-session/d1/7");
    }

    #[test]
    fn test_build_prompt_leaf_has_analysis_scratchpad() {
        let messages = msgs(&[
            ("user", "Fix the bug in auth.rs"),
            ("assistant", "I found the issue"),
        ]);
        let prompt = build_summary_prompt(&messages, 0, None, FallbackLevel::Normal);
        assert!(
            prompt.contains("<analysis>"),
            "leaf prompt should have analysis scratchpad"
        );
        assert!(
            prompt.contains("</analysis>"),
            "leaf prompt should close analysis tag"
        );
        assert!(
            prompt.contains("<summary>"),
            "leaf prompt should have summary section"
        );
    }

    #[test]
    fn test_strip_analysis_block() {
        let input = "Some preamble\n<analysis>\nDetailed reasoning here\n</analysis>\n<summary>\nThe actual summary\n</summary>";
        let stripped = strip_analysis_block(input);
        assert!(!stripped.contains("<analysis>"));
        assert!(!stripped.contains("Detailed reasoning"));
        assert!(stripped.contains("The actual summary"));
    }

    #[test]
    fn test_strip_analysis_block_no_analysis() {
        let input = "Just a plain summary with no analysis block.";
        let stripped = strip_analysis_block(input);
        assert_eq!(stripped, input);
    }

    #[test]
    fn test_all_prompts_contain_identifier_preservation() {
        for depth in 0..=2 {
            let messages = msgs(&[("user", "Fix src/auth.rs commit 0949c9fc")]);
            let prompt = build_summary_prompt(&messages, depth, None, FallbackLevel::Normal);
            assert!(
                prompt.contains("Identifier Preservation"),
                "depth {depth} prompt should contain identifier preservation directive"
            );
        }
    }
}
