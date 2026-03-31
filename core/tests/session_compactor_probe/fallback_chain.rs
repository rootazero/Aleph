//! Fallback chain scenario tests.
//!
//! Tests the three-level fallback strategy: normal LLM -> aggressive LLM ->
//! deterministic truncation.

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
async fn p6_fallback_when_llm_fails() {
    // 1. Create harness with failing LLM
    let h = CompactorProbeHarness::with_failing_llm().await;

    // 2. Make messages and call post_turn_compress
    let messages = CompactorProbeHarness::make_messages(15, "fallback test topic");
    let agent_temp = TempDir::new().unwrap();
    let session_key = SessionKey::main("test-agent");
    let agent = make_agent_with_messages(&agent_temp, &messages, &session_key).await;

    let result = h.compactor.post_turn_compress(&agent, &session_key).await;

    // 3. Assert: no error (fallback succeeded)
    let result = result
        .expect("post_turn_compress should succeed even with failing LLM (deterministic fallback)");

    // 4. Assert: facts created with deterministic content
    let session_id = session_key.to_key_string();
    let facts = h.query_session_facts(&session_id).await;
    assert!(
        !facts.is_empty(),
        "Expected facts to be created via deterministic fallback"
    );
    assert!(
        result.d0_created > 0,
        "Expected d0_created > 0 from deterministic fallback"
    );

    // 5. Assert: metrics.fallback_count > 0
    let fallback_count = h.compactor.metrics().fallback_count.load(Ordering::Relaxed);
    assert!(
        fallback_count > 0,
        "Expected fallback_count > 0, got {}",
        fallback_count
    );

    // 6. Assert: MockLlm was called (it fails, triggering the fallback chain)
    assert!(
        h.mock_llm.call_count() > 0,
        "Expected MockLlm to be called before falling back to deterministic"
    );
}

#[tokio::test]
async fn p6_fallback_facts_have_content() {
    let h = CompactorProbeHarness::with_failing_llm().await;

    let messages = CompactorProbeHarness::make_messages(15, "content verification");
    let agent_temp = TempDir::new().unwrap();
    let session_key = SessionKey::main("test-agent");
    let agent = make_agent_with_messages(&agent_temp, &messages, &session_key).await;

    h.compactor
        .post_turn_compress(&agent, &session_key)
        .await
        .expect("post_turn_compress should succeed");

    let session_id = session_key.to_key_string();
    let facts = h.query_session_facts(&session_id).await;

    // Deterministic fallback should produce non-empty content
    for fact in &facts {
        assert!(
            !fact.content.is_empty(),
            "Fallback fact content should not be empty, path: {}",
            fact.path
        );
    }
}
