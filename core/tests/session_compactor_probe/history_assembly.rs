//! History assembly scenario tests.
//!
//! Tests `prepare_history()` — assembling compressed context from
//! session summaries + fresh tail for the agent loop.
//!
//! `prepare_history` requires `&AgentInstance` and `&SessionKey`, so we
//! create a temporary agent, compress some messages to seed d0 facts,
//! then call `prepare_history` and verify the output contains
//! `<session_context>` XML tags.

use std::sync::atomic::Ordering;
use tempfile::TempDir;

use alephcore::gateway::agent_instance::{AgentInstance, AgentInstanceConfig, MessageRole};
use alephcore::gateway::router::SessionKey;

use super::harness::CompactorProbeHarness;

/// Helper: create a temporary AgentInstance and populate it with messages.
async fn make_agent_with_messages(
    temp_dir: &TempDir,
    messages: &[(String, String)],
    session_key: &SessionKey,
) -> AgentInstance {
    let config = AgentInstanceConfig {
        agent_id: "test-agent".to_string(),
        workspace: temp_dir.path().join("workspace"),
        agent_dir: temp_dir.path().join("agent"),
        ..Default::default()
    };
    let agent = AgentInstance::new(config).expect("Failed to create AgentInstance");
    agent.ensure_session(session_key).await;

    for (role, content) in messages {
        let role = match role.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            _ => MessageRole::User,
        };
        agent.add_message(session_key, role, content).await;
    }

    agent
}

#[tokio::test]
async fn p4_prepare_history_includes_summaries() {
    let h = CompactorProbeHarness::new().await;

    // 1. Create messages and compress to seed d0 summaries
    let messages = CompactorProbeHarness::make_messages(15, "history assembly test");
    let agent_temp = TempDir::new().unwrap();
    let session_key = SessionKey::main("test-agent");
    let agent = make_agent_with_messages(&agent_temp, &messages, &session_key).await;

    let compress_result = h
        .compactor
        .post_turn_compress(&agent, &session_key)
        .await
        .expect("post_turn_compress should not fail");

    assert!(
        compress_result.d0_created > 0,
        "Need d0 summaries for history assembly test"
    );

    // 2. Call prepare_history
    let history = h
        .compactor
        .prepare_history(&agent, &session_key, "continue the discussion", 8000)
        .await;

    // 3. Assert: returned messages contain <session_context> XML tags
    let context_messages: Vec<_> = history
        .iter()
        .filter(|m| m.text_content().contains("<session_context"))
        .collect();

    assert!(
        !context_messages.is_empty(),
        "Expected <session_context> XML blocks in prepare_history output, \
         got {} messages none containing the tag",
        history.len()
    );

    // Verify the XML tags include depth info
    for msg in &context_messages {
        let text = msg.text_content();
        assert!(
            text.contains("depth=\"d0\"") || text.contains("depth=\"d1\"") || text.contains("depth=\"d2\""),
            "session_context block should contain depth attribute, got: {}",
            &text[..text.len().min(200)]
        );
    }

    // 4. Assert: metrics.prepare_history_calls > 0
    let ph_calls = h
        .compactor
        .metrics()
        .prepare_history_calls
        .load(Ordering::Relaxed);
    assert!(
        ph_calls > 0,
        "Expected prepare_history_calls > 0, got {}",
        ph_calls
    );

    // 5. Assert: history also includes fresh tail messages (not just summaries)
    assert!(
        history.len() > context_messages.len(),
        "History should include fresh tail messages beyond just summary blocks"
    );
}

#[tokio::test]
async fn p4_prepare_history_respects_token_budget() {
    let h = CompactorProbeHarness::new().await;

    let messages = CompactorProbeHarness::make_messages(15, "budget test");
    let agent_temp = TempDir::new().unwrap();
    let session_key = SessionKey::main("test-agent");
    let agent = make_agent_with_messages(&agent_temp, &messages, &session_key).await;

    let _compress = h
        .compactor
        .post_turn_compress(&agent, &session_key)
        .await
        .unwrap();

    // Very small budget — should still include fresh tail but may drop summaries
    let small_history = h
        .compactor
        .prepare_history(&agent, &session_key, "test", 50)
        .await;

    // Large budget — should include everything
    let large_history = h
        .compactor
        .prepare_history(&agent, &session_key, "test", 100_000)
        .await;

    // The small-budget history should be no larger than the large-budget one
    assert!(
        small_history.len() <= large_history.len(),
        "Small budget history ({}) should not exceed large budget history ({})",
        small_history.len(),
        large_history.len()
    );
}
