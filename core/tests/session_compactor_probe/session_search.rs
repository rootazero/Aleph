//! Session search scenario tests.
//!
//! Tests querying session-local facts by path prefix and depth,
//! verifying that invalidated facts are properly excluded.

use tempfile::TempDir;

use alephcore::gateway::agent_instance::{AgentInstance, AgentInstanceConfig, MessageRole};
use alephcore::gateway::router::SessionKey;
use alephcore::memory::session_compactor::SessionCompactorConfig;

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
async fn p5_session_facts_queryable() {
    // Use aggressive config to trigger condensation (which invalidates d0 facts)
    let config = SessionCompactorConfig {
        enabled: true,
        fresh_tail_count: 2,
        context_threshold: 0.75,
        leaf_chunk_tokens: 50,  // Very small chunks
        d1_min_fanout: 3,      // Low threshold
        d2_min_fanout: 3,
        max_summary_depth: 2,
        token_estimate_ratio: 3.5,
        session_fact_retention_hours: 24,
        promote_confidence_threshold: 0.8,
    };
    let h = CompactorProbeHarness::with_config(config).await;

    let agent_temp = TempDir::new().unwrap();
    let session_key = SessionKey::main("test-agent");

    // Run multiple compression rounds to create d0 facts and trigger condensation
    for batch in 0..5 {
        let batch_messages =
            CompactorProbeHarness::make_messages(10, &format!("search-batch-{}", batch));
        let agent =
            make_agent_with_messages(&agent_temp, &batch_messages, &session_key).await;

        let _result = h
            .compactor
            .post_turn_compress(&agent, &session_key)
            .await
            .expect("post_turn_compress should not fail");
    }

    let session_id = session_key.to_key_string();

    // 1. Query with query_session_facts (valid only)
    let valid_facts = h.query_session_facts(&session_id).await;

    // 2. Query with query_all_session_facts (includes invalid)
    let all_facts = h.query_all_session_facts(&session_id).await;

    // Both queries should return non-empty results
    assert!(
        !valid_facts.is_empty(),
        "Expected valid facts to exist"
    );
    assert!(
        !all_facts.is_empty(),
        "Expected all facts to exist"
    );

    // 3. After condensation, all >= valid (condensation invalidates source facts)
    assert!(
        all_facts.len() >= valid_facts.len(),
        "All facts ({}) should be >= valid facts ({})",
        all_facts.len(),
        valid_facts.len()
    );

    // 4. Verify some facts were invalidated by condensation
    let invalid_count = all_facts.len() - valid_facts.len();
    assert!(
        invalid_count > 0,
        "Expected some invalidated facts after condensation, \
         all={} valid={}",
        all_facts.len(),
        valid_facts.len()
    );

    // 5. Verify count_facts_at_depth works
    let d0_valid = h.count_facts_at_depth(&session_id, 0).await;
    let d1_valid = h.count_facts_at_depth(&session_id, 1).await;
    // At least one depth should have valid facts
    assert!(
        d0_valid > 0 || d1_valid > 0,
        "Expected valid facts at d0 ({}) or d1 ({})",
        d0_valid,
        d1_valid
    );
}

#[tokio::test]
async fn p5_different_sessions_are_isolated() {
    let h = CompactorProbeHarness::new().await;

    let agent_temp = TempDir::new().unwrap();
    let session_a = SessionKey::main("agent-a");
    let session_b = SessionKey::main("agent-b");

    // Populate session A
    let msgs_a = CompactorProbeHarness::make_messages(15, "topic alpha");
    let agent_a = make_agent_with_messages(&agent_temp, &msgs_a, &session_a).await;
    h.compactor
        .post_turn_compress(&agent_a, &session_a)
        .await
        .unwrap();

    let id_a = session_a.to_key_string();
    let id_b = session_b.to_key_string();

    let facts_a = h.query_session_facts(&id_a).await;
    let facts_b = h.query_session_facts(&id_b).await;

    assert!(!facts_a.is_empty(), "Session A should have facts");
    assert!(facts_b.is_empty(), "Session B should have no facts (never compressed)");
}
