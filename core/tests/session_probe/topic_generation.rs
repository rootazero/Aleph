//! P3: Topic generation tests.
//!
//! Covers LLM-driven topic summarisation, fallback on LLM failure,
//! and topic persistence in session metadata.

use std::sync::Arc;

use super::harness::SessionProbeHarness;
use super::mock_llm::MockLlmProvider;

// =============================================================================
// P3 tests
// =============================================================================

#[tokio::test]
async fn p3_01_new_command_generates_topic_with_sufficient_history() {
    let llm = Arc::new(MockLlmProvider::new("Rust并发讨论"));
    let mut h = SessionProbeHarness::with_llm(Some(llm.clone()));
    h.register_channel("ch-topic").await;
    h.register_agent("main", None).await;

    // Seed 4+ messages so history.len() >= 2
    h.seed_dm_messages("main", &[
        ("user", "What is Rust concurrency?"),
        ("assistant", "Rust uses ownership and borrowing for thread safety."),
        ("user", "How does async work?"),
        ("assistant", "Async in Rust uses futures and an executor."),
    ]).await;

    // Send /new to trigger topic generation
    h.send_message("ch-topic", "/new").await;

    // LLM should have been called for topic generation
    assert!(llm.call_count() > 0, "LLM should be called for topic generation");

    // Old session should be closed with a topic
    assert!(h.is_dm_session_closed("main").await, "Old session should be closed");
    let topic = h.get_dm_topic("main").await;
    assert_eq!(topic, Some("Rust并发讨论".to_string()));
}

#[tokio::test]
async fn p3_02_new_command_skips_topic_with_short_history() {
    let llm = Arc::new(MockLlmProvider::new("should not be called"));
    let mut h = SessionProbeHarness::with_llm(Some(llm.clone()));
    h.register_channel("ch-short").await;
    h.register_agent("main", None).await;

    // Seed only 1 message — history.len() < 2 triggers skip
    h.seed_dm_messages("main", &[
        ("user", "Hello"),
    ]).await;

    h.send_message("ch-short", "/new").await;

    // LLM should NOT be called (history too short)
    assert_eq!(llm.call_count(), 0, "LLM should not be called with < 2 messages");

    // Topic should be None
    let topic = h.get_dm_topic("main").await;
    assert!(topic.is_none(), "Topic should be None for short history");
}

#[tokio::test]
async fn p3_03_llm_failure_produces_no_topic_gracefully() {
    let llm = Arc::new(MockLlmProvider::failing());
    let mut h = SessionProbeHarness::with_llm(Some(llm.clone()));
    h.register_channel("ch-fail").await;
    h.register_agent("main", None).await;

    // Seed sufficient messages
    h.seed_dm_messages("main", &[
        ("user", "Tell me about memory safety"),
        ("assistant", "Rust prevents memory bugs at compile time."),
        ("user", "What about lifetimes?"),
        ("assistant", "Lifetimes ensure references are valid."),
    ]).await;

    // Should not panic even though LLM fails
    h.send_message("ch-fail", "/new").await;

    // Session should still be closed, but topic is None
    assert!(h.is_dm_session_closed("main").await, "Session should be closed despite LLM failure");
    let topic = h.get_dm_topic("main").await;
    assert!(topic.is_none(), "Topic should be None when LLM fails");
}

#[tokio::test]
async fn p3_04_new_command_sends_confirmation_reply() {
    let llm = Arc::new(MockLlmProvider::new("测试主题"));
    let mut h = SessionProbeHarness::with_llm(Some(llm));
    h.register_channel("ch-reply").await;
    h.register_agent("main", None).await;

    // Seed sufficient messages for topic generation
    h.seed_dm_messages("main", &[
        ("user", "Question one"),
        ("assistant", "Answer one"),
        ("user", "Question two"),
        ("assistant", "Answer two"),
    ]).await;

    h.send_message("ch-reply", "/new").await;

    h.assert_reply_contains("新对话已开始");
}

#[tokio::test]
async fn p3_05_switch_closes_old_session() {
    let llm = Arc::new(MockLlmProvider::new("切换前对话"));
    let mut h = SessionProbeHarness::with_llm(Some(llm));
    h.register_channel("ch-switch").await;
    h.register_agent("main", None).await;
    h.register_agent("helper", None).await;

    // Seed messages for the main agent's session
    h.seed_dm_messages("main", &[
        ("user", "Help me with Rust"),
        ("assistant", "Sure, what do you need?"),
        ("user", "Explain traits"),
        ("assistant", "Traits define shared behavior."),
    ]).await;

    // Switch to helper agent
    h.send_message("ch-switch", "/switch helper").await;

    // Old session (main agent) should be closed
    assert!(
        h.is_dm_session_closed("main").await,
        "Old session should be closed after /switch"
    );
}

#[test]
fn p3_06_truncate_handles_multibyte_utf8() {
    // Test that char_indices-based truncation works on Chinese characters
    // without panicking (would panic if using byte slicing &s[..n])
    let chinese = "这是一个测试字符串用来验证多字节字符截断";

    // Truncate to 5 chars — should produce first 5 Chinese characters
    let truncated = match chinese.char_indices().nth(5) {
        Some((idx, _)) => &chinese[..idx],
        None => chinese,
    };
    assert_eq!(truncated, "这是一个测");

    // Truncate to more than length — should return full string
    let full = match chinese.char_indices().nth(100) {
        Some((idx, _)) => &chinese[..idx],
        None => chinese,
    };
    assert_eq!(full, chinese);

    // Empty string — should not panic
    let empty = "";
    let result = match empty.char_indices().nth(5) {
        Some((idx, _)) => &empty[..idx],
        None => empty,
    };
    assert_eq!(result, "");

    // Mixed ASCII + CJK
    let mixed = "abc你好def世界";
    let truncated_mixed = match mixed.char_indices().nth(5) {
        Some((idx, _)) => &mixed[..idx],
        None => mixed,
    };
    assert_eq!(truncated_mixed, "abc你好");
}
