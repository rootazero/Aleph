//! Tool compaction scenario tests.
//!
//! Tests that large tool results are compressed into concise summaries
//! while preserving essential information.

use alephcore::memory::session_compactor::context_window::estimate_total_tokens;
use alephcore::memory::session_compactor::tool_compactor::compact_if_needed;
use alephcore::providers::message::UnifiedMessage;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a conversation with `count` tool-call rounds (user → tool_result → assistant).
///
/// Each tool result contains a large file-read output (~200 lines) to ensure
/// the total token count is high enough to trigger compaction.
fn make_large_tool_conversation(count: usize) -> Vec<UnifiedMessage> {
    let mut messages = Vec::with_capacity(count * 3);
    for i in 0..count {
        messages.push(UnifiedMessage::user(format!("Read source file #{}", i + 1)));

        // Large file-read output — each line is ~60 chars, 200 lines ≈ 12 000 chars ≈ 3 400 tokens
        let file_content: String = (0..200)
            .map(|l| {
                format!(
                    "fn component_{}_line_{}_impl() {{ /* body */ }}\n",
                    i + 1,
                    l
                )
            })
            .collect();
        messages.push(UnifiedMessage::tool_result(
            format!("call_{}", i + 1),
            "Read",
            file_content,
            false,
        ));

        messages.push(UnifiedMessage::assistant(format!(
            "I've read source file #{} and understand its structure.",
            i + 1
        )));
    }
    messages
}

// ---------------------------------------------------------------------------
// Scenario 1 — ToolCompactor Trigger
// ---------------------------------------------------------------------------

/// P1: Large tool results are compressed when context exceeds the token budget.
///
/// Build 20 rounds of tool calls (user → Read result → assistant), set a
/// very small budget (5 000 tokens), call `compact_if_needed`, and assert
/// that old tool results have been replaced by `[Read file, …]` summaries.
#[test]
fn p1_tool_results_compressed_when_over_threshold() {
    let mut messages = make_large_tool_conversation(20);

    // Confirm the conversation is actually large before compaction.
    let initial_tokens = estimate_total_tokens(&messages, 3.5);
    assert!(
        initial_tokens > 5_000,
        "pre-condition: conversation must exceed budget; got {initial_tokens} tokens"
    );

    // Use fresh_tail_count=3 so the last round is protected.
    compact_if_needed(&mut messages, 5_000, 0.75, 3.5, 3);

    // At least one of the non-tail tool results must have been compressed.
    let protected_start = messages.len().saturating_sub(3);
    let compressed_count = messages[..protected_start]
        .iter()
        .filter(|m| {
            if let Some((_, content)) = m.tool_result_info() {
                content.starts_with("[Read file,")
            } else {
                false
            }
        })
        .count();

    assert!(
        compressed_count > 0,
        "expected at least one compressed tool result in the compressible zone; \
         got 0 out of {} candidates",
        protected_start
    );
}

/// P1: No compaction occurs when token usage is well below the budget.
///
/// Build a small conversation, set a very high budget, and verify that
/// `compact_if_needed` is a no-op (all content preserved verbatim).
#[test]
fn p1_no_compaction_under_threshold() {
    let mut messages = vec![
        UnifiedMessage::user("hello"),
        UnifiedMessage::tool_result("c1", "Read", "fn main() {}\n", false),
        UnifiedMessage::assistant("Done."),
    ];

    let original_content = {
        let (_, c) = messages[1].tool_result_info().unwrap();
        c
    };

    // Budget = 1 000 000 tokens → nothing should trigger.
    compact_if_needed(&mut messages, 1_000_000, 0.75, 3.5, 2);

    let (_, after_content) = messages[1].tool_result_info().unwrap();
    assert_eq!(
        after_content, original_content,
        "tool result must not be modified when under threshold"
    );
}

/// P1: The last `fresh_tail_count` messages are never compressed.
///
/// Place a large tool result inside the fresh tail window and verify it
/// remains untouched even when the overall budget is far exceeded.
#[test]
fn p1_fresh_tail_preserved() {
    // Build an old section (will be compressed) + a fresh tail section (must not be touched).
    let old_section = make_large_tool_conversation(10); // indices 0..29

    let large_fresh_output: String = (0..200)
        .map(|l| format!("fresh_fn_line_{}_untouched() {{ /* body */ }}\n", l))
        .collect();

    let fresh_tail = vec![
        UnifiedMessage::user("fresh request"),
        UnifiedMessage::tool_result("fresh_call", "Read", &large_fresh_output, false),
        UnifiedMessage::assistant("fresh reply"),
    ];

    let mut messages: Vec<UnifiedMessage> = old_section
        .into_iter()
        .chain(fresh_tail.into_iter())
        .collect();

    // Verify pre-condition: conversation is large.
    let initial_tokens = estimate_total_tokens(&messages, 3.5);
    assert!(
        initial_tokens > 5_000,
        "pre-condition: need large conversation; got {initial_tokens}"
    );

    // Protect the last 3 messages (the fresh tail).
    let fresh_tail_count = 3;
    compact_if_needed(&mut messages, 5_000, 0.75, 3.5, fresh_tail_count);

    // The fresh tail tool result must be exactly unchanged.
    let tail_idx = messages.len() - 2; // second from end (tool_result before final assistant)
    let (_, tail_content) = messages[tail_idx].tool_result_info().unwrap();
    assert!(
        tail_content.contains("fresh_fn_line_"),
        "fresh tail tool result must not be compressed; got: {tail_content:.80}"
    );
}
