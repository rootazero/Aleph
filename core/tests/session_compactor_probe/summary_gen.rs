//! Summary generation scenario tests.
//!
//! Tests d0 leaf summary creation from chunked conversation messages.

use std::sync::atomic::Ordering;
use tempfile::TempDir;

use alephcore::gateway::agent_instance::{AgentInstance, AgentInstanceConfig, MessageRole};
use alephcore::gateway::router::SessionKey;
use alephcore::memory::context::{FactSource, MemoryScope};

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
            "system" => MessageRole::System,
            "tool" => MessageRole::Tool,
            _ => MessageRole::User,
        };
        agent.add_message(session_key, role, content).await;
    }

    agent
}

#[tokio::test]
async fn p2_post_turn_creates_d0_summaries() {
    // 1. Create harness
    let h = CompactorProbeHarness::new().await;

    // 2. Make 30 conversation messages (15 turns)
    let messages = CompactorProbeHarness::make_messages(15, "session compaction");

    // 3. Create an AgentInstance with those messages
    let agent_temp = TempDir::new().unwrap();
    let session_key = SessionKey::main("test-agent");
    let agent = make_agent_with_messages(&agent_temp, &messages, &session_key).await;

    // 4. Call post_turn_compress
    let result = h
        .compactor
        .post_turn_compress(&agent, &session_key)
        .await
        .expect("post_turn_compress should not fail");

    // 5. Assert: d0 summaries were created
    assert!(
        result.d0_created > 0,
        "Expected CompressResult.d0_created > 0, got {}",
        result.d0_created
    );

    // 6. Assert: metrics.d0_summaries_created > 0
    let d0_count = h
        .compactor
        .metrics()
        .d0_summaries_created
        .load(Ordering::Relaxed);
    assert!(
        d0_count > 0,
        "Expected d0_summaries_created metric > 0, got {}",
        d0_count
    );

    // 7. Query LanceDB for ALL SessionLocal facts (including those invalidated by
    //    d0→d1 condensation that may happen in the same call).
    let session_id = session_key.to_key_string();
    let all_facts = h.query_all_session_facts(&session_id).await;

    assert!(
        !all_facts.is_empty(),
        "Expected session facts to exist in LanceDB"
    );

    // 8. Verify d0 facts were created (may be invalidated by condensation)
    let d0_facts: Vec<_> = all_facts
        .iter()
        .filter(|f| f.path.contains("/d0/"))
        .collect();
    assert!(
        !d0_facts.is_empty(),
        "Expected d0 facts in LanceDB, got paths: {:?}",
        all_facts.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    // 9. All facts should have correct scope and fact_source
    for fact in &all_facts {
        assert_eq!(
            fact.scope,
            MemoryScope::SessionLocal,
            "Fact scope should be SessionLocal"
        );
        assert_eq!(
            fact.fact_source,
            FactSource::SessionCompressed,
            "Fact source should be SessionCompressed"
        );
    }

    // 10. Assert: MockLlm was called (call_count > 0)
    assert!(
        h.mock_llm.call_count() > 0,
        "Expected MockLlm to be called for summary generation"
    );
}

#[tokio::test]
async fn p2_no_compression_when_too_few_messages() {
    let h = CompactorProbeHarness::new().await;

    // Only 2 messages — below the fresh_tail_count (4), nothing to compress
    let messages = CompactorProbeHarness::make_messages(1, "short conversation");

    let agent_temp = TempDir::new().unwrap();
    let session_key = SessionKey::main("test-agent");
    let agent = make_agent_with_messages(&agent_temp, &messages, &session_key).await;

    let result = h
        .compactor
        .post_turn_compress(&agent, &session_key)
        .await
        .expect("post_turn_compress should not fail");

    assert_eq!(
        result.d0_created, 0,
        "No d0 summaries should be created for too-few messages"
    );

    let session_id = session_key.to_key_string();
    let facts = h.query_session_facts(&session_id).await;
    assert!(
        facts.is_empty(),
        "No facts should exist for too-few messages"
    );
}
